//! The C ABI of the WebAssembly build (`wasm32-unknown-unknown`), for a host
//! that loads the module without any JS glue (the fluxpkg.dev Worker):
//!
//! ```text
//! fdoc_alloc(len: u32) -> *mut u8
//! fdoc_free(ptr: *mut u8, len: u32)
//! fdoc_generate(in_ptr: *const u8, in_len: u32, out_len: *mut u32) -> *mut u8
//! fdoc_version() -> *const u8          // NUL-terminated
//! fdoc_last_panic() -> *const u8       // NUL-terminated, empty if none
//! ```
//!
//! The host allocates `in_len` bytes with `fdoc_alloc`, writes the UTF-8
//! JSON input there (see [`crate::input_from_json`]), calls
//! `fdoc_generate`, reads `*out_len` bytes of JSON output at the returned
//! pointer, and frees both buffers with `fdoc_free` and the lengths they
//! were made with. The output is `{ "pages": [...], "warnings": [...] }` or
//! `{ "error": "..." }`.
//!
//! A panic never crosses the boundary as an unwind: `fdoc_generate` catches
//! it and answers `{ "error": ... }`. `wasm32-unknown-unknown` builds with
//! `panic = "abort"` unless std is rebuilt with unwinding, and then a panic
//! traps instead (a `WebAssembly.RuntimeError` in the host); the panic hook
//! has already stored the message for `fdoc_last_panic`, and the host must
//! drop the instance, whose state is no longer trustworthy.
//!
//! The module imports nothing.

use std::cell::RefCell;
use std::panic;
use std::sync::Once;

thread_local! {
    /// The message of the last panic, NUL-terminated.
    static LAST_PANIC: RefCell<Vec<u8>> = RefCell::new(vec![0]);
}

static HOOK: Once = Once::new();

fn install_hook() {
    HOOK.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let mut message = format!("fdoc panicked: {info}").into_bytes();
            message.retain(|&b| b != 0);
            message.push(0);
            let _ = LAST_PANIC.try_with(|p| {
                if let Ok(mut p) = p.try_borrow_mut() {
                    *p = message;
                }
            });
        }));
    });
}

/// Allocates `len` bytes for the host to write into; free with `fdoc_free`.
/// A zero length gives a dangling, non-null pointer that needs no freeing.
#[unsafe(no_mangle)]
pub extern "C" fn fdoc_alloc(len: u32) -> *mut u8 {
    let mut buf = std::mem::ManuallyDrop::new(Vec::<u8>::with_capacity(len as usize));
    buf.as_mut_ptr()
}

/// Frees a buffer from `fdoc_alloc` or `fdoc_generate`.
///
/// # Safety
///
/// `ptr` and `len` must be a pointer and the length it was allocated with.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fdoc_free(ptr: *mut u8, len: u32) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: the caller passes a buffer of this module's allocator with
    // the capacity it was made with (exact in `fdoc_alloc` and `into_raw`).
    unsafe { drop(Vec::from_raw_parts(ptr, 0, len as usize)) };
}

/// Moves `s` to a buffer the host frees with `fdoc_free(ptr, len)`.
fn into_raw(s: String, out_len: *mut u32) -> *mut u8 {
    // A boxed slice has exactly `len` bytes of capacity.
    let bytes = Box::into_raw(s.into_bytes().into_boxed_slice());
    if !out_len.is_null() {
        // SAFETY: the caller passes a writable `u32` in this module's memory.
        unsafe { *out_len = bytes.len() as u32 };
    }
    bytes.cast::<u8>()
}

/// Generates documentation from the JSON input at `in_ptr`; see the module
/// documentation.
///
/// # Safety
///
/// `in_ptr` must point to `in_len` readable bytes and `out_len` to a
/// writable `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fdoc_generate(in_ptr: *const u8, in_len: u32, out_len: *mut u32) -> *mut u8 {
    install_hook();
    let input: &[u8] = if in_ptr.is_null() || in_len == 0 {
        &[]
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { std::slice::from_raw_parts(in_ptr, in_len as usize) }
    };
    let result = panic::catch_unwind(|| match std::str::from_utf8(input) {
        Ok(text) => crate::generate_json(text),
        Err(e) => crate::error_to_json(&format!("invalid input: not UTF-8: {e}")),
    });
    let json = result.unwrap_or_else(|payload| {
        let message = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into());
        crate::error_to_json(&format!("fdoc panicked: {message}"))
    });
    into_raw(json, out_len)
}

/// The crate version, NUL-terminated.
#[unsafe(no_mangle)]
pub extern "C" fn fdoc_version() -> *const u8 {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr()
}

/// The message of the last panic, NUL-terminated; empty when there was
/// none. Valid until the next panic.
#[unsafe(no_mangle)]
pub extern "C" fn fdoc_last_panic() -> *const u8 {
    LAST_PANIC.with(|p| p.borrow().as_ptr())
}
