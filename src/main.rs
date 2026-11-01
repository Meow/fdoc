//! fdoc - generates HTML documentation, or a JSON document of the same
//! model, from the doc comments of a Lua code base, with its Lumen
//! templates and NL files.

//!
//! This is the directory walker around the library (`src/lib.rs`): it
//! reads the config and the source files, calls `fdoc::generate_with`, and
//! writes the pages.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fdoc::{Config, Format, Input, is_source_file};

const USAGE: &str = "\
fdoc - HTML and JSON documentation generator for Lua and NL doc comments

Usage: fdoc [OPTIONS] <SOURCE_DIR>

Arguments:
  <SOURCE_DIR>            Directory to scan recursively for .lua, .lumen, .nl and .nlua files

Options:
  -o, --output <DIR>      Output directory (default: <SOURCE_DIR>/docs); - writes the JSON
                          document to standard output
  -f, --format <FORMAT>   html (default): the pages; json: the whole model as <DIR>/fdoc.json
      --config <FILE>     Layout and project settings (default: <SOURCE_DIR>/.fdoc.yml if it exists)
      --title <NAME>      Project name (default: from the config or packagespec.lua, else the directory name)
      --source-url <URL>  Base URL that source paths are appended to for \"view source\" links
      --exclude <NAME>    Directory name to skip anywhere, or a path containing / (such as ./lib or
                          a/b.lua) relative to <SOURCE_DIR> to skip; repeatable (default: docs, .git)
      --documented-only   Only include functions that have a doc comment
      --clean             Delete the output directory before generating
  -q, --quiet             Only print errors
  -h, --help              Print this help
  -V, --version           Print the version
";

struct Args {
    source: PathBuf,
    output: Option<PathBuf>,
    format: Format,
    config: Option<PathBuf>,
    title: Option<String>,
    source_url: Option<String>,
    excludes: Vec<String>,
    documented_only: bool,
    clean: bool,
    quiet: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut source = None;
    let mut out = Args { source: PathBuf::new(), output: None, format: Format::Html, config: None, title: None, source_url: None, excludes: vec!["docs".into(), ".git".into()], documented_only: false, clean: false, quiet: false };

    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} requires a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("fdoc {}", fdoc::VERSION);
                std::process::exit(0);
            }
            "-o" | "--output" => out.output = Some(PathBuf::from(value("--output")?)),
            "-f" | "--format" => {
                out.format = match value("--format")?.as_str() {
                    "html" => Format::Html,
                    "json" => Format::Json,
                    other => return Err(format!("unknown format: {other} (expected html or json)")),
                }
            }
            "--config" => out.config = Some(PathBuf::from(value("--config")?)),
            "--title" => out.title = Some(value("--title")?),
            "--source-url" => out.source_url = Some(value("--source-url")?),
            "--exclude" => out.excludes.push(value("--exclude")?),
            "--documented-only" => out.documented_only = true,
            "--clean" => out.clean = true,
            "-q" | "--quiet" => out.quiet = true,
            s if s.starts_with('-') => return Err(format!("unknown option: {s}")),
            _ => {
                if source.is_some() {
                    return Err("only one source directory can be given".into());
                }
                source = Some(PathBuf::from(a));
            }
        }
    }
    out.source = source.ok_or_else(|| "missing <SOURCE_DIR>".to_string())?;
    Ok(out)
}

/// Reads the `--config` file, or `.fdoc.yml` in the source directory when
/// there is one.
fn load_config(args: &Args) -> Result<(Option<PathBuf>, Config), String> {
    let path = match &args.config {
        Some(p) => p.clone(),
        None => {
            let p = args.source.join(".fdoc.yml");
            if !p.is_file() {
                return Ok((None, Config::default()));
            }
            p
        }
    };
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let config = Config::from_yaml(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((Some(path), config))
}

/// An exclude entry with a `/` other than a trailing one (`./lib`,
/// `a/b.lua`) names a directory or file by its path relative to the source
/// root; otherwise (`lib`, `lib/`) it is a directory name that is skipped
/// anywhere.
fn is_excluded(rel: &str, name: &str, is_dir: bool, excludes: &[String]) -> bool {
    excludes.iter().any(|e| {
        let e = e.trim_end_matches('/');
        if e.contains('/') {
            e.trim_start_matches("./").trim_start_matches('/') == rel
        } else {
            is_dir && !e.is_empty() && e == name
        }
    })
}

/// Collects the source files below `root` (Lua files, Lumen templates and
/// NL files), sorted, as `/` separated paths relative to it.
fn collect_source_files(root: &Path, excludes: &[String], skip: Option<&Path>) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let file_type = entry.file_type().map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if file_type.is_dir() {
                if name.starts_with('.') || is_excluded(&rel, &name, true, excludes) || skip.is_some_and(|s| same_path(s, &path)) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && is_source_file(&name) && !is_excluded(&rel, &name, false, excludes) {
                files.push(rel);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Reads a metadata file (`packagespec.lua`, `plugin.yml`, ...) for the
/// library: its text if it is a file below `source`, else `None`.
fn read_metadata(source: &Path, rel: &str) -> Result<Option<String>, String> {
    let path = source.join(rel);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(Some(String::from_utf8_lossy(&text).into_owned()))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn run(args: Args) -> Result<(), String> {
    let source = &args.source;
    if !source.is_dir() {
        return Err(format!("{} is not a directory", source.display()));
    }
    let output = args.output.clone().unwrap_or_else(|| source.join("docs"));
    let to_stdout = output.as_os_str() == "-";
    if to_stdout && args.format != Format::Json {
        return Err("only --format json can be written to standard output".into());
    }

    let (config_path, config) = load_config(&args)?;
    if !args.quiet {
        let name = config_path.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
        for warning in &config.warnings {
            eprintln!("warning: {name}: {warning}");
        }
    }
    let excludes: Vec<String> = args.excludes.iter().chain(&config.exclude).cloned().collect();

    let files = collect_source_files(source, &excludes, (!to_stdout).then_some(output.as_path()))?;
    if files.is_empty() {
        return Err(format!("no .lua, .lumen, .nl or .nlua files found in {}", source.display()));
    }
    // Only the files inside a group of the layout are read and scanned; the
    // library looks up the `packagespec.lua` and `plugin.yml` files itself.
    let total = files.len();
    let in_layout = fdoc::layout_filter(Some(&config));
    let files: Vec<String> = files.into_iter().filter(|f| in_layout(f)).collect();
    if files.is_empty() {
        return Err(format!("none of the {total} source files in {} matches a group of the layout", source.display()));
    }
    let skipped = total - files.len();

    let mut sources = Vec::with_capacity(files.len());
    for rel in files {
        let path = source.join(&rel);
        let text = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        sources.push(fdoc::File { path: rel, text: String::from_utf8_lossy(&text).into_owned() });
    }

    let dir_name = source.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let input = Input { title: args.title.clone(), version: None, source_url: args.source_url.clone(), files: sources, config: Some(config), fallback_title: dir_name, documented_only: args.documented_only, format: args.format };
    let generated = fdoc::generate_with(input, &mut |rel: &str| read_metadata(source, rel)).map_err(|e| match e {
        fdoc::Error::Metadata { path, message } => format!("{}: {message}", source.join(path).display()),
        fdoc::Error::NoSourceFiles => format!("no .lua, .lumen, .nl or .nlua files found in {}", source.display()),
        fdoc::Error::NoFilesInLayout { total } => format!("none of the {total} source files in {} matches a group of the layout", source.display()),
        fdoc::Error::Read(message) => message,
        e @ fdoc::Error::InvalidPath(_) => e.to_string(),
    })?;
    let stats = generated.stats;
    if !args.quiet {
        for warning in &generated.warnings {
            eprintln!("warning: {warning}");
        }
    }

    if args.clean && !to_stdout && output.exists() {
        // Both exist at this point, so canonicalising only fails on a
        // permission problem; refusing is the safe answer then.
        let holds_source = match (source.canonicalize(), output.canonicalize()) {
            (Ok(s), Ok(o)) => s.starts_with(o),
            _ => true,
        };
        if holds_source {
            return Err(format!("refusing to clean {}: it contains the source directory", output.display()));
        }
        fs::remove_dir_all(&output).map_err(|e| format!("cannot remove {}: {e}", output.display()))?;
    }

    let pages = generated.pages;
    let written = match args.format {
        Format::Html => {
            for page in &pages {
                let path = output.join(&page.path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
                fs::write(&path, &page.content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            }
            format!("Wrote {} pages to {}", pages.len(), output.display())
        }
        Format::Json if to_stdout => {
            let doc = pages.first().map_or("", |p| p.content.as_str());
            std::io::stdout().write_all(doc.as_bytes()).map_err(|e| format!("cannot write to standard output: {e}"))?;
            "Wrote the JSON document to standard output".to_string()
        }
        Format::Json => {
            fs::create_dir_all(&output).map_err(|e| format!("cannot create {}: {e}", output.display()))?;
            let path = output.join("fdoc.json");
            let doc = pages.first().map_or("", |p| p.content.as_str());
            fs::write(&path, doc).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            format!("Wrote {}", path.display())
        }
    };

    if !args.quiet {
        let (scanned, modules, functions, documented) = (stats.scanned, stats.modules, stats.functions, stats.documented);
        let mut report = format!("Scanned {scanned} files: {modules} modules, {functions} functions ({documented} documented).\n");
        if skipped > 0 {
            report.push_str(&format!("Skipped {skipped} files that match no group of the layout.\n"));
        }
        report.push_str(&written);
        // The JSON document owns standard output when it is written there.
        if to_stdout {
            eprintln!("{report}");
        } else {
            println!("{report}");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_names_and_paths() {
        let excludes = vec!["thirdparty".to_string(), "./gamemodes/a/lib/".to_string(), "x/skip.lua".to_string()];
        assert!(is_excluded("lib/thirdparty", "thirdparty", true, &excludes));
        assert!(!is_excluded("lib/thirdparty", "thirdparty", false, &excludes), "names only match directories");
        assert!(is_excluded("gamemodes/a/lib", "lib", true, &excludes));
        assert!(!is_excluded("gamemodes/b/lib", "lib", true, &excludes));
        assert!(is_excluded("x/skip.lua", "skip.lua", false, &excludes));
    }

    #[test]
    fn reads_lua_lumen_and_nl_files() {
        for name in ["a.lua", "b.lumen", "c.nl", "d.nlua"] {
            assert!(is_source_file(name), "{name}");
        }
        assert!(!is_source_file("glua.d.nl"), "declaration files are not read");
        assert!(!is_source_file("notes.md"));
    }

    #[test]
    fn root_relative_excludes_are_paths() {
        let excludes = vec!["./lib".to_string(), "./packagespec.lua".to_string(), "vendor/".to_string()];
        assert!(is_excluded("lib", "lib", true, &excludes));
        assert!(!is_excluded("gamemodes/a/lib", "lib", true, &excludes), "`./lib` is the root `lib` only");
        assert!(is_excluded("packagespec.lua", "packagespec.lua", false, &excludes), "a path can name a file");
        assert!(!is_excluded("packages/a/packagespec.lua", "packagespec.lua", false, &excludes));
        assert!(is_excluded("a/vendor", "vendor", true, &excludes), "a trailing `/` keeps a name");
    }
}
