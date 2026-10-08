//! Walks the tokens of one Lua file and collects the declarations that can
//! be documented, together with the documentation comment that precedes
//! each of them, plus the hooks the file runs and adds, its file-local
//! objects, realm blocks and metadata.

use std::collections::BTreeSet;
use std::ops::Range;

pub use crate::doc::Realm;
use crate::doc::{self, DocBlock};
use crate::lexer::{tokenize, Tok, Token};

/// Calls that run a hook by name: the callee path as written, and how many
/// arguments sit between the hook name and the hook's own arguments (the
/// gamemode table of `hook.Call`).
pub const HOOK_CALLERS: [(&str, usize); 7] = [
    ("hook.Run", 0),
    ("hook.Call", 1),
    ("Plugin.call", 0),
    ("Plugin.Call", 0),
    ("plugin.Call", 0),
    ("cw.plugin:Call", 0),
    ("Clockwork.plugin:Call", 0),
];

/// Longest source rendering kept for `LocalTable::init` and `HookCall::args`.
const MAX_SOURCE_LEN: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sep {
    /// `Owner.name` — a function stored in a table.
    Dot,
    /// `Owner:name` — a method.
    Colon,
    /// `name` — a global function.
    None,
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    /// Owner path, e.g. `ActiveRecord.Base` or `player_meta`; empty for globals.
    pub owner: String,
    pub sep: Sep,
    pub name: String,
    pub params: Vec<String>,
    pub line: usize,
    pub doc: Option<DocBlock>,
    /// Category active at the declaration point (from `@category`).
    pub category: Option<String>,
    /// Realm implied by the enclosing `if SERVER` / `if CLIENT` blocks of the
    /// file only; `Shared` outside of them.
    pub realm: Realm,
    /// Index into `FileScan::locals` of the most recent top-level local
    /// named like the first component of `owner` (`PANEL` for `PANEL:Init`).
    pub local_table: Option<usize>,
}

impl FunctionDecl {
    /// `owner:name`, `owner.name` or `name`, with the owner as written.
    pub fn qualified(&self) -> String {
        match self.sep {
            Sep::None => self.name.clone(),
            Sep::Dot => format!("{}.{}", self.owner, self.name),
            Sep::Colon => format!("{}:{}", self.owner, self.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub name: String,
    pub extends: Option<String>,
    pub line: usize,
    pub doc: Option<DocBlock>,
}

#[derive(Debug, Clone, Default)]
pub struct Category {
    pub name: String,
    pub description: String,
}

/// A top-level `local NAME = <expr>`, such as `local PANEL = {}` or
/// `local COMMAND = cw.command:New('A')`.
#[derive(Debug, Clone)]
pub struct LocalTable {
    pub name: String,
    pub line: usize,
    /// Compact rendering of the right-hand side, cut after ~80 characters.
    pub init: String,
    /// The doc block directly above the `local` line.
    pub doc: Option<DocBlock>,
    /// Name from the first `vgui.Register('name', NAME, ...)` that follows
    /// this declaration, before `NAME` is declared again.
    pub register_name: Option<String>,
}

/// A call that runs a hook, such as `hook.Run('Name', a, b)`.
#[derive(Debug, Clone)]
pub struct HookCall {
    pub name: String,
    /// Source rendering of each argument after the hook name.
    pub args: Vec<String>,
    pub line: usize,
    /// Realm of the enclosing `if SERVER` / `if CLIENT` blocks.
    pub realm: Realm,
    /// The doc block directly above the statement the call starts on.
    pub doc: Option<DocBlock>,
    /// Qualified name of the innermost enclosing named function, as written.
    pub caller: Option<String>,
}

/// A `hook.Add('Name', id, handler)` call.
#[derive(Debug, Clone)]
pub struct HookAdd {
    pub name: String,
    /// The identifier, when it is a string literal.
    pub id: Option<String>,
    /// Parameters of an inline handler function; empty otherwise.
    pub params: Vec<String>,
    pub line: usize,
    pub realm: Realm,
    pub doc: Option<DocBlock>,
}

#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub functions: Vec<FunctionDecl>,
    pub classes: Vec<ClassDecl>,
    /// `local x = FindMetaTable('Y')` style aliases: (x, Y).
    pub meta_aliases: Vec<(String, String)>,
    /// Categories declared in this file, in order.
    pub categories: Vec<Category>,
    /// `PLUGIN:set_name(...)` / `SetName`, `set_description` / `SetDescription`,
    /// `set_author` / `SetAuthor`, `set_global` / `SetGlobal` / `SetGlobalAlias`.
    pub plugin_name: Option<String>,
    pub plugin_description: Option<String>,
    pub plugin_author: Option<String>,
    pub plugin_global: Option<String>,
    /// `vgui.Register('name', PANEL)`; the first one of the file.
    pub vgui_name: Option<String>,
    /// `s.field = 'value'` pairs inside `Package:describe`.
    pub spec: Vec<(String, String)>,
    /// Top-level `local NAME = <expr>` declarations, except metatable aliases.
    pub locals: Vec<LocalTable>,
    /// Every name a `local` statement declares, at any depth: variables
    /// (`local a, b = ...`) and `local function` names.
    pub local_names: BTreeSet<String>,
    /// Hook calls with a string literal name (see `HOOK_CALLERS`).
    pub hook_calls: Vec<HookCall>,
    /// `hook.Add` calls with a string literal name.
    pub hook_adds: Vec<HookAdd>,
    /// The first free-standing doc block before the first declaration.
    pub file_doc: Option<DocBlock>,
    /// Libraries declared by `library.New('name', parent)` or `mod 'Name'`:
    /// (dotted qualified name, line).
    pub libraries: Vec<(String, usize)>,
    /// Top-level `Owner.field = 'string'` assignments: (owner path, field, value).
    pub fields: Vec<(String, String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Function,
    Other,
}

/// An open block on the scanner's stack.
struct Frame {
    kind: Block,
    /// Realm of the current branch of a realm-conditional `if`.
    realm: Option<Realm>,
    /// Realm of the later `elseif` / `else` branches, implied by the realm
    /// conditions before them.
    rest: Option<Realm>,
    /// Qualified name of a named function declaration.
    name: Option<String>,
}

impl Frame {
    fn function(name: Option<String>) -> Frame {
        Frame { kind: Block::Function, realm: None, rest: None, name }
    }

    fn other(realm: Option<Realm>) -> Frame {
        Frame { kind: Block::Other, realm, rest: realm.map(Realm::opposite), name: None }
    }
}

/// A doc block directly above a statement that is not consumed yet.
struct Held {
    doc: DocBlock,
    /// Line and token index of the statement's first token.
    line: usize,
    index: usize,
    /// No declaration precedes it, so it can become the file doc.
    file_doc: bool,
}

struct Scanner {
    out: FileScan,
    stack: Vec<Frame>,
    category: Option<String>,
    held: Option<Held>,
    /// A function, class or local declaration has been seen.
    seen_decl: bool,
}

/// Returns the realm a source path implies (the path is `/` separated and
/// relative to the source root): `sv_`, `cl_` and `sh_` file name prefixes,
/// `init.lua`, `cl_init.lua` and `shared.lua`, then the closest directory
/// named `server`, `client` or `shared`; `Shared` otherwise.
pub fn path_realm(path: &str) -> Realm {
    let mut parts = path.split('/').rev();
    let file = parts.next().unwrap_or("").to_ascii_lowercase();
    match file.as_str() {
        "init.lua" => return Realm::Server,
        "cl_init.lua" => return Realm::Client,
        "shared.lua" => return Realm::Shared,
        _ => {}
    }
    for (prefix, realm) in [("sv_", Realm::Server), ("cl_", Realm::Client), ("sh_", Realm::Shared)] {
        if file.starts_with(prefix) {
            return realm;
        }
    }
    for dir in parts {
        match dir.to_ascii_lowercase().as_str() {
            "server" => return Realm::Server,
            "client" => return Realm::Client,
            "shared" => return Realm::Shared,
            _ => {}
        }
    }
    Realm::Shared
}

/// Returns true when the comment token can start a documentation block.
fn is_doc_start(t: &Token) -> bool {
    match &t.tok {
        Tok::Comment { text, long: false } if t.line_start => {
            // `---`, but not a separator line such as `------------`.
            let text = text.trim_end();
            text == "-" || (text.starts_with('-') && !text.chars().all(|c| c == '-'))
        }
        _ => false,
    }
}

pub fn scan(src: &str) -> FileScan {
    let toks = tokenize(src);
    let mut s = Scanner { out: FileScan::default(), stack: Vec::new(), category: None, held: None, seen_decl: false };

    // Pending doc block: raw comment texts and the line of the last one.
    let mut pending: Vec<String> = Vec::new();
    let mut pending_last_line = 0usize;

    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];

        if s.held.as_ref().is_some_and(|h| h.line != t.line) {
            s.release_held();
        }

        if let Tok::Comment { text, long } = &t.tok {
            if !pending.is_empty() && !*long && t.line_start && t.line == pending_last_line + 1 {
                pending.push(text.clone());
                pending_last_line = t.line;
            } else if is_doc_start(t) {
                if !pending.is_empty() {
                    s.standalone(doc::parse(&pending), !s.seen_decl);
                }
                pending = vec![text.clone()];
                pending_last_line = t.line;
            } else if !pending.is_empty() {
                s.standalone(doc::parse(&pending), !s.seen_decl);
                pending.clear();
            }
            i += 1;
            continue;
        }

        // Non-comment token: the doc block attaches only if it is directly
        // above; it is held until a declaration or hook on this line takes it.
        if !pending.is_empty() {
            let block = doc::parse(&pending);
            pending.clear();
            if t.line == pending_last_line + 1 {
                s.held = Some(Held { doc: block, line: t.line, index: i, file_doc: !s.seen_decl });
            } else {
                s.standalone(block, !s.seen_decl);
            }
        }

        let in_function = s.in_function();

        // Block structure.
        if let Some(name) = t.name() {
            match name {
                "function" => {
                    // `function Name(...)` declarations are handled here;
                    // the body is tracked either way.
                    let doc = s.take_decl_doc(i);
                    match parse_function_decl(&toks, i) {
                        Some((decl, next)) => {
                            s.stack.push(Frame::function(Some(decl.qualified())));
                            if !in_function {
                                s.push_function(decl, doc);
                            }
                            i = next;
                        }
                        None => {
                            s.stack.push(Frame::function(None));
                            i += 1;
                        }
                    }
                    continue;
                }
                "if" => {
                    s.stack.push(Frame::other(condition_realm(&toks, i + 1)));
                    i += 1;
                    continue;
                }
                "elseif" => {
                    let realm = condition_realm(&toks, i + 1);
                    if let Some(frame) = s.stack.last_mut().filter(|f| f.kind == Block::Other) {
                        frame.realm = realm.or(frame.rest);
                        if realm.is_some() {
                            frame.rest = realm.map(Realm::opposite);
                        }
                    }
                    i += 1;
                    continue;
                }
                "else" => {
                    if let Some(frame) = s.stack.last_mut().filter(|f| f.kind == Block::Other) {
                        frame.realm = frame.rest;
                    }
                    i += 1;
                    continue;
                }
                "do" | "repeat" => {
                    s.stack.push(Frame::other(None));
                    i += 1;
                    continue;
                }
                "end" | "until" => {
                    s.stack.pop();
                    i += 1;
                    continue;
                }
                "local" => {
                    s.out.local_names.extend(local_names(&toks, i));
                    // `local x = FindMetaTable('Y')`, `local function`, `local x = <expr>`.
                    if let Some((alias, target)) = parse_meta_alias(&toks, i) {
                        s.out.meta_aliases.push((alias, target));
                    } else if toks.get(i + 1).is_some_and(|n| n.is_name("function")) {
                        // Local functions are private; skip the name but track the body.
                        s.take_decl_doc(i);
                        let name = toks.get(i + 2).and_then(|n| n.name()).map(str::to_string);
                        s.stack.push(Frame::function(name));
                        if !in_function {
                            s.seen_decl = true;
                        }
                        i = skip_to_params_end(&toks, i + 2);
                        continue;
                    } else if !in_function && let Some((name, rhs)) = parse_local(&toks, i) {
                        // A hook run on the right-hand side keeps the doc block.
                        let doc = if hook_call_at(&toks, rhs).is_some() { None } else { s.take_decl_doc(i).filter(|d| !d.ignore) };
                        let end = statement_end(&toks, rhs);
                        s.out.locals.push(LocalTable { name, line: t.line, init: render(&toks, rhs..end), doc, register_name: None });
                        s.seen_decl = true;
                    }
                    i += 1;
                    continue;
                }
                "class" => {
                    if let Some((mut decl, next)) = parse_class_decl(&toks, i) {
                        decl.doc = s.take_decl_doc(i);
                        s.out.classes.push(decl);
                        s.seen_decl = true;
                        i = next;
                        continue;
                    }
                }
                _ => {}
            }
        }

        // `Name.path = function(...)`.
        if !in_function && let Some((decl, next)) = parse_assigned_function(&toks, i) {
            let doc = s.take_decl_doc(i);
            s.stack.push(Frame::function(Some(decl.qualified())));
            s.push_function(decl, doc);
            i = next;
            continue;
        }

        // Hooks run and added; the arguments are scanned as usual afterwards.
        if let Some((name, args)) = hook_call_at(&toks, i) {
            let doc = s.take_stmt_doc(t.line);
            let call = HookCall { name, args, line: t.line, realm: s.realm(), doc, caller: s.caller() };
            s.out.hook_calls.push(call);
            i += 1;
            continue;
        }
        if let Some((name, id, params)) = hook_add_at(&toks, i) {
            let doc = s.take_stmt_doc(t.line);
            s.out.hook_adds.push(HookAdd { name, id, params, line: t.line, realm: s.realm(), doc });
            i += 1;
            continue;
        }

        if let Some((library, next)) = parse_library(&toks, i) {
            s.out.libraries.push((library, t.line));
            i = next;
            continue;
        }

        // Metadata calls.
        if let Some(next) = parse_metadata(&toks, i, &mut s.out, in_function) {
            i = next;
            continue;
        }

        i += 1;
    }

    s.release_held();
    if !pending.is_empty() {
        s.standalone(doc::parse(&pending), !s.seen_decl);
    }

    s.out
}

impl Scanner {
    fn in_function(&self) -> bool {
        self.stack.iter().any(|f| f.kind == Block::Function)
    }

    /// Realm of the innermost realm-conditional block.
    fn realm(&self) -> Realm {
        self.stack.iter().rev().find_map(|f| f.realm).unwrap_or(Realm::Shared)
    }

    /// Name of the innermost enclosing function, if it is a named one.
    fn caller(&self) -> Option<String> {
        self.stack.iter().rev().find(|f| f.kind == Block::Function).and_then(|f| f.name.clone())
    }

    /// Takes the held doc block if the declaration at token `i` is the
    /// first token after it.
    fn take_decl_doc(&mut self, i: usize) -> Option<DocBlock> {
        if self.held.as_ref().is_some_and(|h| h.index == i) { self.held.take().map(|h| h.doc) } else { None }
    }

    /// Takes the held doc block if it sits directly above `line`.
    fn take_stmt_doc(&mut self, line: usize) -> Option<DocBlock> {
        if self.held.as_ref().is_some_and(|h| h.line == line) { self.held.take().map(|h| h.doc) } else { None }
    }

    fn release_held(&mut self) {
        if let Some(h) = self.held.take() {
            self.standalone(h.doc, h.file_doc);
        }
    }

    /// A doc block that is not attached to a declaration: it can switch the
    /// current category, or become the file doc when it comes before the
    /// first declaration. Everything else is dropped.
    fn standalone(&mut self, block: DocBlock, file_doc: bool) {
        if let Some(name) = block.category {
            self.out.categories.push(Category { name: name.clone(), description: block.description });
            self.category = Some(name);
        } else if file_doc && !block.ignore && self.out.file_doc.is_none() {
            self.out.file_doc = Some(block);
        }
    }

    fn push_function(&mut self, mut decl: FunctionDecl, doc: Option<DocBlock>) {
        self.seen_decl = true;
        if doc.as_ref().is_some_and(|d| d.ignore) {
            return;
        }
        let first = decl.owner.split('.').next().unwrap_or("");
        decl.local_table = if first.is_empty() { None } else { self.out.locals.iter().rposition(|l| l.name == first) };
        decl.realm = self.realm();
        decl.doc = doc;
        decl.category = self.category.clone();
        self.out.functions.push(decl);
    }
}

/// Parses `function a.b:c(x, y)` starting at the `function` token.
/// Returns the declaration and the index after `)`. Anonymous functions
/// return None.
fn parse_function_decl(toks: &[Token], i: usize) -> Option<(FunctionDecl, usize)> {
    let line = toks[i].line;
    let mut j = i + 1;
    let mut parts: Vec<String> = Vec::new();
    let mut sep = Sep::None;

    let first = toks.get(j)?.name()?.to_string();
    parts.push(first);
    j += 1;
    loop {
        let t = toks.get(j)?;
        if t.is_sym(".") {
            let n = toks.get(j + 1)?.name()?.to_string();
            parts.push(n);
            j += 2;
        } else if t.is_sym(":") {
            let n = toks.get(j + 1)?.name()?.to_string();
            parts.push(n);
            sep = Sep::Colon;
            j += 2;
            break;
        } else {
            break;
        }
    }
    if sep == Sep::None && parts.len() > 1 {
        sep = Sep::Dot;
    }
    let name = parts.pop()?;
    let owner = parts.join(".");
    let (params, next) = parse_params(toks, j)?;
    Some((new_decl(owner, sep, name, params, line), next))
}

fn new_decl(owner: String, sep: Sep, name: String, params: Vec<String>, line: usize) -> FunctionDecl {
    FunctionDecl { owner, sep, name, params, line, doc: None, category: None, realm: Realm::Shared, local_table: None }
}

/// Parses `Name.path = function(...)` / `Name.path = function(...)`.
fn parse_assigned_function(toks: &[Token], i: usize) -> Option<(FunctionDecl, usize)> {
    if !toks[i].line_start {
        return None;
    }
    let line = toks[i].line;
    let mut parts = vec![toks[i].name()?.to_string()];
    let mut j = i + 1;
    while toks.get(j)?.is_sym(".") {
        parts.push(toks.get(j + 1)?.name()?.to_string());
        j += 2;
    }
    if !toks.get(j)?.is_sym("=") || !toks.get(j + 1)?.is_name("function") {
        return None;
    }
    let name = parts.pop()?;
    let owner = parts.join(".");
    let sep = if owner.is_empty() { Sep::None } else { Sep::Dot };
    let (params, next) = parse_params(toks, j + 2)?;
    Some((new_decl(owner, sep, name, params, line), next))
}

fn parse_params(toks: &[Token], i: usize) -> Option<(Vec<String>, usize)> {
    if !toks.get(i)?.is_sym("(") {
        return None;
    }
    let mut params = Vec::new();
    let mut j = i + 1;
    loop {
        let t = toks.get(j)?;
        match &t.tok {
            Tok::Sym(s) if s == ")" => return Some((params, j + 1)),
            Tok::Sym(s) if s == "," => {}
            Tok::Sym(s) if s == "..." => params.push("...".to_string()),
            Tok::Name(n) => params.push(n.clone()),
            _ => return None,
        }
        j += 1;
    }
}

/// Skips `name(...)` of a local function, returning the index after `)`.
fn skip_to_params_end(toks: &[Token], mut i: usize) -> usize {
    while i < toks.len() && !toks[i].is_sym(")") {
        i += 1;
    }
    i + 1
}

/// `class 'Name'` or `class('Name')`, optionally followed by `extends 'Base'`.
fn parse_class_decl(toks: &[Token], i: usize) -> Option<(ClassDecl, usize)> {
    let line = toks[i].line;
    let mut j = i + 1;
    let paren = toks.get(j)?.is_sym("(");
    if paren {
        j += 1;
    }
    let name = toks.get(j)?.string()?.to_string();
    j += 1;
    if paren && toks.get(j).is_some_and(|t| t.is_sym(")")) {
        j += 1;
    }
    let mut extends = None;
    if toks.get(j).is_some_and(|t| t.is_name("extends")) {
        let mut k = j + 1;
        if toks.get(k).is_some_and(|t| t.is_sym("(")) {
            k += 1;
        }
        if let Some(base) = toks.get(k).and_then(|t| t.string()) {
            extends = Some(base.to_string());
            k += 1;
            if toks.get(k).is_some_and(|t| t.is_sym(")")) {
                k += 1;
            }
            j = k;
        }
    }
    Some((ClassDecl { name, extends, line, doc: None }, j))
}

/// `local x = FindMetaTable('Y')` -> (x, Y); `local x = debug.getmetatable(0)` -> (x, Number).
fn parse_meta_alias(toks: &[Token], i: usize) -> Option<(String, String)> {
    let alias = toks.get(i + 1)?.name()?.to_string();
    if !toks.get(i + 2)?.is_sym("=") {
        return None;
    }
    let t = toks.get(i + 3)?;
    if t.is_name("FindMetaTable") && toks.get(i + 4)?.is_sym("(") {
        let target = toks.get(i + 5)?.string()?.to_string();
        return Some((alias, target));
    }
    if t.is_name("debug") && toks.get(i + 4)?.is_sym(".") && toks.get(i + 5)?.is_name("getmetatable") {
        let arg = toks.get(i + 7)?;
        let target = match &arg.tok {
            Tok::Number(_) => "Number",
            Tok::Str(_) => "String",
            _ => return None,
        };
        return Some((alias, target.to_string()));
    }
    None
}

/// The names the `local` statement at `i` declares: `local function f`
/// gives `f`, `local a, b <const> = ...` gives `a` and `b`.
fn local_names(toks: &[Token], i: usize) -> Vec<String> {
    if toks.get(i + 1).is_some_and(|t| t.is_name("function")) {
        return toks.get(i + 2).and_then(|t| t.name()).filter(|n| !is_keyword(n)).map(|n| vec![n.to_string()]).unwrap_or_default();
    }
    let mut names = Vec::new();
    let mut j = i + 1;
    while let Some(name) = toks.get(j).and_then(|t| t.name()).filter(|n| !is_keyword(n)) {
        names.push(name.to_string());
        j += 1;
        if toks.get(j).is_some_and(|t| t.is_sym("<")) {
            j += 3;
        }
        if !toks.get(j).is_some_and(|t| t.is_sym(",")) {
            break;
        }
        j += 1;
    }
    names
}

/// `local NAME = ...` -> (NAME, index of the first token of the right-hand side).
fn parse_local(toks: &[Token], i: usize) -> Option<(String, usize)> {
    let name = toks.get(i + 1)?.name()?;
    if is_keyword(name) || !toks.get(i + 2)?.is_sym("=") || toks.get(i + 3).is_none_or(|t| matches!(t.tok, Tok::Comment { .. })) {
        return None;
    }
    Some((name.to_string(), i + 3))
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "and" | "break" | "continue" | "do" | "else" | "elseif" | "end" | "false" | "for" | "function" | "goto" | "if" | "in" | "local" | "nil" | "not" | "or" | "repeat" | "return" | "then" | "true" | "until" | "while"
    )
}

/// How much a token changes the nesting depth: brackets and the keywords
/// that open (`function`, `if`, `do`, `repeat`) and close (`end`, `until`)
/// blocks.
fn depth_change(t: &Token) -> isize {
    match &t.tok {
        Tok::Sym(s) if matches!(s.as_str(), "(" | "[" | "{") => 1,
        Tok::Sym(s) if matches!(s.as_str(), ")" | "]" | "}") => -1,
        Tok::Name(n) if matches!(n.as_str(), "function" | "if" | "do" | "repeat") => 1,
        Tok::Name(n) if matches!(n.as_str(), "end" | "until") => -1,
        _ => 0,
    }
}

/// True when `t` cannot start a statement and so continues the expression
/// before it on the previous line.
fn continues_expression(prev: &Token, t: &Token) -> bool {
    let binary = |t: &Token| match &t.tok {
        Tok::Sym(s) => !matches!(s.as_str(), ")" | "]" | "}" | ";"),
        Tok::Name(n) => matches!(n.as_str(), "and" | "or" | "not"),
        _ => false,
    };
    binary(prev) || (binary(t) && !t.is_sym("(") && !t.is_sym("::") && !t.is_name("not"))
}

/// Index of the token after the expression statement starting at `start`.
fn statement_end(toks: &[Token], start: usize) -> usize {
    let mut depth = 0isize;
    let mut prev: Option<&Token> = None;
    let mut j = start;
    while j < toks.len() {
        let t = &toks[j];
        if matches!(t.tok, Tok::Comment { .. }) {
            j += 1;
            continue;
        }
        if depth == 0 && let Some(p) = prev {
            let block_end = matches!(t.name(), Some("end" | "else" | "elseif" | "until"));
            if t.is_sym(";") || block_end || (t.line_start && !continues_expression(p, t)) {
                break;
            }
        }
        depth += depth_change(t);
        if depth < 0 {
            break;
        }
        prev = Some(t);
        j += 1;
    }
    j
}

/// Splits the arguments of the call whose `(` is at `open` into token
/// ranges. Returns the ranges and the index after `)`.
fn call_args(toks: &[Token], open: usize) -> Option<(Vec<Range<usize>>, usize)> {
    if !toks.get(open)?.is_sym("(") {
        return None;
    }
    let mut args = Vec::new();
    let mut depth = 0isize;
    let mut start = open + 1;
    let mut j = open + 1;
    while j < toks.len() {
        let t = &toks[j];
        if depth == 0 && (t.is_sym(",") || t.is_sym(")")) {
            if j > start || t.is_sym(",") {
                args.push(start..j);
            }
            if t.is_sym(")") {
                return Some((args, j + 1));
            }
            start = j + 1;
        } else {
            depth += depth_change(t);
        }
        j += 1;
    }
    None
}

/// The arguments of a call at token `i` to the dotted / colon path `path`
/// (`hook.Run`): the call may use parentheses or a single string literal.
fn call_at(toks: &[Token], i: usize, path: &str) -> Option<Vec<Range<usize>>> {
    if i > 0 && (toks[i - 1].is_sym(".") || toks[i - 1].is_sym(":") || toks[i - 1].is_name("function")) {
        return None;
    }
    let mut j = i;
    let mut rest = path;
    loop {
        let end = rest.find(['.', ':']).unwrap_or(rest.len());
        if !toks.get(j)?.is_name(&rest[..end]) {
            return None;
        }
        j += 1;
        if end == rest.len() {
            break;
        }
        if !toks.get(j)?.is_sym(&rest[end..end + 1]) {
            return None;
        }
        j += 1;
        rest = &rest[end + 1..];
    }
    let t = toks.get(j)?;
    if t.string().is_some() {
        return Some(std::iter::once(j..j + 1).collect());
    }
    call_args(toks, j).map(|(args, _)| args)
}

/// The string literal that makes up a whole argument, if any.
fn literal(toks: &[Token], arg: &Range<usize>) -> Option<String> {
    if arg.len() != 1 {
        return None;
    }
    toks[arg.start].string().map(str::to_string)
}

/// A hook call at token `i` (see `HOOK_CALLERS`): its name and arguments.
fn hook_call_at(toks: &[Token], i: usize) -> Option<(String, Vec<String>)> {
    toks.get(i)?.name()?;
    for (path, skip) in HOOK_CALLERS {
        if let Some(args) = call_at(toks, i, path) {
            let name = literal(toks, args.first()?)?;
            let rendered = args.iter().skip(1 + skip).map(|a| render(toks, a.clone())).collect();
            return Some((name, rendered));
        }
    }
    None
}

/// `hook.Add('Name', id, handler)` at token `i`: name, literal id, and the
/// parameters of an inline handler function.
fn hook_add_at(toks: &[Token], i: usize) -> Option<(String, Option<String>, Vec<String>)> {
    let args = call_at(toks, i, "hook.Add")?;
    let name = literal(toks, args.first()?)?;
    let id = args.get(1).and_then(|a| literal(toks, a));
    let params = match args.get(2) {
        Some(a) if toks[a.start].is_name("function") => parse_params(toks, a.start + 1).map(|(p, _)| p).unwrap_or_default(),
        _ => Vec::new(),
    };
    Some((name, id, params))
}

/// `library.New('name', parent)` -> `parent.name` (`name` for `_G`), and
/// Flux's `mod 'Name'` -> `Name`, with `::` written as `.`. Returns the
/// name and the index to continue from.
fn parse_library(toks: &[Token], i: usize) -> Option<(String, usize)> {
    let t = &toks[i];
    if t.is_name("mod") && t.line_start {
        let (j, paren) = if toks.get(i + 1)?.is_sym("(") { (i + 2, true) } else { (i + 1, false) };
        let name = toks.get(j)?.string()?.replace("::", ".");
        if paren && !toks.get(j + 1)?.is_sym(")") {
            return None;
        }
        return Some((name, j + 1 + paren as usize));
    }
    if !t.is_name("library") || !toks.get(i + 1)?.is_sym(".") || !toks.get(i + 2)?.is_name("New") {
        return None;
    }
    let (args, next) = call_args(toks, i + 3)?;
    let name = literal(toks, args.first()?)?;
    let parent = args.get(1).map(|a| render(toks, a.clone())).unwrap_or_default();
    let qualified = if parent.is_empty() || parent == "_G" { name } else { format!("{parent}.{name}") };
    Some((qualified, next))
}

/// The realm an `if` / `elseif` condition (the tokens from `start` to
/// `then`) checks for: `SERVER` or `CLIENT`, flipped by a `not` or `!`
/// directly before it. None when it mentions neither or both.
fn condition_realm(toks: &[Token], start: usize) -> Option<Realm> {
    let mut found: Option<Realm> = None;
    for j in start..toks.len() {
        let t = &toks[j];
        if t.is_name("then") {
            break;
        }
        let realm = match t.name() {
            Some("SERVER") => Realm::Server,
            Some("CLIENT") => Realm::Client,
            _ => continue,
        };
        let prev = &toks[j - 1];
        if prev.is_sym(".") || prev.is_sym(":") {
            continue;
        }
        let realm = if prev.is_name("not") || prev.is_sym("!") { realm.opposite() } else { realm };
        match found {
            Some(r) if r != realm => return None,
            _ => found = Some(realm),
        }
    }
    found
}

/// Renders the tokens in `range` as compact source: strings in single
/// quotes, one space between tokens except around `.`, `:` and brackets,
/// before `,` and after unary operators. Cut after `MAX_SOURCE_LEN`
/// characters.
fn render(toks: &[Token], range: Range<usize>) -> String {
    let mut out = String::new();
    let mut prev: Option<&Token> = None;
    let mut unary = false;
    for t in &toks[range] {
        let text = match &t.tok {
            Tok::Comment { .. } => continue,
            Tok::Name(s) | Tok::Number(s) | Tok::Sym(s) => s.clone(),
            Tok::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n")),
        };
        if let Some(p) = prev
            && !unary
            && space_between(p, t)
        {
            out.push(' ');
        }
        // `-` is unary at the start or after an operator or keyword.
        unary = t.is_sym("#") || t.is_sym("!") || (t.is_sym("-") && prev.is_none_or(|p| !ends_operand(p)));
        out.push_str(&text);
        prev = Some(t);
        if out.chars().count() > MAX_SOURCE_LEN {
            let cut: String = out.chars().take(MAX_SOURCE_LEN).collect();
            return format!("{}…", cut.trim_end());
        }
    }
    out
}

/// True when `t` can end an operand, so that `(` after it is a call and
/// `-` after it is a subtraction.
fn ends_operand(t: &Token) -> bool {
    match &t.tok {
        Tok::Name(n) => !is_keyword(n) || matches!(n.as_str(), "end" | "nil" | "true" | "false"),
        Tok::Str(_) | Tok::Number(_) => true,
        Tok::Sym(s) => matches!(s.as_str(), ")" | "]" | "}" | "..."),
        Tok::Comment { .. } => false,
    }
}

fn space_between(prev: &Token, t: &Token) -> bool {
    let sym = |t: &Token, set: &[&str]| matches!(&t.tok, Tok::Sym(s) if set.contains(&s.as_str()));
    if sym(prev, &[".", ":", "(", "["]) || sym(t, &[".", ":", ")", "]", ",", ";"]) {
        return false;
    }
    if prev.is_sym("{") && t.is_sym("}") {
        return false;
    }
    // A call or index such as `f(x)`, `t[k]` or `function(a)`.
    if sym(t, &["(", "["]) {
        return !(ends_operand(prev) || prev.is_name("function"));
    }
    true
}

/// Recognises `PLUGIN:set_x('...')` / `PLUGIN:SetX('...')`,
/// `vgui.Register('...', PANEL)`, `s.field = '...'` package spec assignments
/// and top-level `Owner.field = '...'` assignments. Returns the index to
/// continue from.
fn parse_metadata(toks: &[Token], i: usize, out: &mut FileScan, in_function: bool) -> Option<usize> {
    let t = &toks[i];
    let name = t.name()?;

    if name == "PLUGIN" && toks.get(i + 1)?.is_sym(":") {
        let method = toks.get(i + 2)?.name()?;
        if !toks.get(i + 3)?.is_sym("(") {
            return None;
        }
        let value = toks.get(i + 4)?.string()?.to_string();
        match method {
            "set_name" | "SetName" => out.plugin_name = Some(value),
            "set_description" | "SetDescription" => out.plugin_description = Some(value),
            "set_author" | "SetAuthor" => out.plugin_author = Some(value),
            "set_global" | "SetGlobal" | "SetGlobalAlias" => out.plugin_global = Some(value),
            _ => return None,
        }
        return Some(i + 5);
    }

    if name == "vgui" && toks.get(i + 1)?.is_sym(".") && toks.get(i + 2)?.is_name("Register") && toks.get(i + 3)?.is_sym("(") {
        let value = toks.get(i + 4)?.string()?.to_string();
        if out.vgui_name.is_none() {
            out.vgui_name = Some(value.clone());
        }
        if toks.get(i + 5).is_some_and(|t| t.is_sym(","))
            && let Some(object) = toks.get(i + 6).and_then(|t| t.name())
            && toks.get(i + 7).is_some_and(|t| t.is_sym(",") || t.is_sym(")"))
            && let Some(local) = out.locals.iter_mut().rev().find(|l| l.name == object)
            && local.register_name.is_none()
        {
            local.register_name = Some(value);
        }
        return Some(i + 5);
    }

    if !t.line_start || !toks.get(i + 1)?.is_sym(".") {
        return None;
    }

    // `Owner.path.field = 'value'`, where the value is a whole string literal.
    let mut parts = vec![name];
    let mut j = i + 1;
    while toks.get(j)?.is_sym(".") {
        parts.push(toks.get(j + 1)?.name()?);
        j += 2;
    }
    if !toks.get(j)?.is_sym("=") {
        return None;
    }
    let value = toks.get(j + 1)?.string()?;
    if toks.get(j + 2).is_some_and(|t| matches!(&t.tok, Tok::Sym(s) if s != ";")) {
        return None;
    }
    let field = parts.pop()?;

    // `s.name = 'value'` inside `Package:describe(function(s) ... end)`.
    if parts.len() == 1 && matches!(field, "name" | "version" | "date" | "summary" | "description" | "author" | "website" | "license" | "global") {
        out.spec.push((field.to_string(), value.to_string()));
    }
    if !in_function {
        out.fields.push((parts.join("."), field.to_string(), value.to_string()));
    }
    Some(j + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_declarations_with_docs() {
        let scan = scan(
            "--- Doc A.\n-- @param x [Number]\nfunction A.B:method(x, ...)\n  local function inner() end\n  function nested() end\nend\n\n--- Floating.\n\nfunction global_fn(a)\nend\n\nlocal function private() end\n\nif SERVER then\n  --- Doc C.\n  function C.fn(y)\n  end\nend\n\n--- Doc D.\nD.fn = function(z) end\n--- @ignore\nfunction hidden() end\n",
        );
        let names: Vec<String> = scan.functions.iter().map(|f| format!("{}{}", f.owner, f.name)).collect();
        assert_eq!(names, vec!["A.Bmethod", "global_fn", "Cfn", "Dfn"]);
        let f = &scan.functions[0];
        assert_eq!(f.sep, Sep::Colon);
        assert_eq!(f.params, vec!["x", "..."]);
        assert_eq!(f.doc.as_ref().unwrap().summary(), "Doc A.");
        assert!(scan.functions[1].doc.is_none(), "floating comment must not attach");
        assert_eq!(scan.functions[2].doc.as_ref().unwrap().summary(), "Doc C.");
        assert_eq!(scan.functions[3].sep, Sep::Dot);
        assert_eq!(scan.functions[3].params, vec!["z"]);
        assert!(scan.file_doc.is_none(), "a block after a declaration is not the file doc");
    }

    #[test]
    fn scans_categories_classes_and_metadata() {
        let scan = scan(
            "--- A class.\nclass 'Foo::Bar' extends 'Base'\nlocal player_meta = FindMetaTable('Player')\nPLUGIN:set_name('Crosshair')\nPLUGIN:set_global('Hints')\nvgui.Register('fl_button', PANEL)\n\n--- @category [Query Engine]\n-- Query functions.\n\nfunction Foo.Bar:where()\nend\n\n  s.name        = 'Flux'\n  s.summary     = 'A framework.'\n",
        );
        assert_eq!(scan.classes[0].name, "Foo::Bar");
        assert_eq!(scan.classes[0].extends.as_deref(), Some("Base"));
        assert_eq!(scan.classes[0].doc.as_ref().unwrap().summary(), "A class.");
        assert_eq!(scan.meta_aliases, vec![("player_meta".to_string(), "Player".to_string())]);
        assert_eq!(scan.plugin_name.as_deref(), Some("Crosshair"));
        assert_eq!(scan.plugin_global.as_deref(), Some("Hints"));
        assert_eq!(scan.vgui_name.as_deref(), Some("fl_button"));
        assert_eq!(scan.categories[0].name, "Query Engine");
        assert_eq!(scan.categories[0].description, "Query functions.");
        assert_eq!(scan.functions[0].category.as_deref(), Some("Query Engine"));
        assert_eq!(scan.spec, vec![("name".to_string(), "Flux".to_string()), ("summary".to_string(), "A framework.".to_string())]);
        assert!(scan.locals.is_empty(), "metatable aliases are not locals");
    }

    #[test]
    fn parses_demo_fixture() {
        let scan = scan(include_str!("../tests/fixtures/demo.lua"));
        assert_eq!(scan.functions.len(), 2);
        let test = &scan.functions[0];
        assert_eq!(test.name, "test");
        assert_eq!(test.params, vec!["a", "b", "c"]);
        let doc = test.doc.as_ref().unwrap();
        assert_eq!(doc.params.len(), 3);
        assert_eq!(doc.returns.len(), 2);
        assert_eq!(doc.see.len(), 2);
        let foo = &scan.functions[1];
        let doc = foo.doc.as_ref().unwrap();
        assert_eq!(doc.variants.len(), 2);
        assert_eq!(doc.variants[0].params.len(), 2);
        assert_eq!(doc.variants[1].params.len(), 1);
        assert_eq!(doc.returns[0].ty.as_deref(), Some("Nil"));
    }

    #[test]
    fn derives_realms_from_paths() {
        assert_eq!(path_realm("packages/flow/lib/sv_player.lua"), Realm::Server);
        assert_eq!(path_realm("lib/stdlib/sh_stdlib.lua"), Realm::Shared);
        assert_eq!(path_realm("plugins/cl_crosshair.lua"), Realm::Client);
        assert_eq!(path_realm("core/entities/entities/cw_item/init.lua"), Realm::Server);
        assert_eq!(path_realm("core/entities/entities/cw_item/cl_init.lua"), Realm::Client);
        assert_eq!(path_realm("core/entities/entities/cw_item/shared.lua"), Realm::Shared);
        assert_eq!(path_realm("server/client/x.lua"), Realm::Client);
        assert_eq!(path_realm("client/shared/x.lua"), Realm::Shared);
        assert_eq!(path_realm("server/cl_x.lua"), Realm::Client);
        assert_eq!(path_realm("lib/pipeline.lua"), Realm::Shared);
        assert_eq!(path_realm("server.lua"), Realm::Shared);
    }

    #[test]
    fn tracks_realm_blocks() {
        let scan = scan(
            "function a() end
if SERVER then
  function b() end
  if CLIENT then function c() end end
  if x then function d() end end
else
  function e() end
end
if !SERVER then function f() end else function g() end end
if not CLIENT and y then
  function h() end
elseif SERVER then
  function i() end
else
  function j() end
end
if SERVER then
  hook.Run('Run', function() if x then function k() end end end)
elseif z then
  function l() end
end
if SERVER or CLIENT then function m() end end
do if CLIENT then function n() end end end
",
        );
        let realms: Vec<(String, Realm)> = scan.functions.iter().map(|f| (f.name.clone(), f.realm)).collect();
        let expected = [
            ("a", Realm::Shared),
            ("b", Realm::Server),
            ("c", Realm::Client),
            ("d", Realm::Server),
            ("e", Realm::Client),
            ("f", Realm::Client),
            ("g", Realm::Server),
            ("h", Realm::Server),
            ("i", Realm::Server),
            ("j", Realm::Client),
            ("l", Realm::Client),
            ("m", Realm::Shared),
            ("n", Realm::Client),
        ];
        assert_eq!(realms, expected.iter().map(|(n, r)| (n.to_string(), *r)).collect::<Vec<_>>());
        assert_eq!(scan.hook_calls[0].realm, Realm::Server);
    }

    #[test]
    fn records_local_tables() {
        let scan = scan(
            "--[[ License. --]]

library.New('currency', cw)

local stored = cw.currency.stored or {}

--- A currency.
-- @module [Currency]
local CLASS_TABLE = { __index = CLASS_TABLE }

function CLASS_TABLE:Create(name) end

local PANEL = {}
function PANEL:Init() local inner = 1 end
vgui.Register('cw.characterMenu', PANEL, 'DPanel')
vgui.Register('cw.other', PANEL, 'DPanel')

local PANEL = {
  a = 1,
}
function PANEL:Paint(w, h) end
vgui.Register('cw.characterList', PANEL, 'EditablePanel')

local COMMAND = cw.command:New('CharFallOver')
COMMAND.tip = '#Commands_FallOverDesc'
function COMMAND:OnRun(player, arguments) end
function stored.thing() end
local x = 1
local y
local function z() end
local long = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
",
        );
        let locals: Vec<(&str, &str, Option<&str>)> = scan.locals.iter().map(|l| (l.name.as_str(), l.init.as_str(), l.register_name.as_deref())).collect();
        assert_eq!(locals[..6], [
            ("stored", "cw.currency.stored or {}", None),
            ("CLASS_TABLE", "{ __index = CLASS_TABLE }", None),
            ("PANEL", "{}", Some("cw.characterMenu")),
            ("PANEL", "{ a = 1, }", Some("cw.characterList")),
            ("COMMAND", "cw.command:New('CharFallOver')", None),
            ("x", "1", None),
        ]);
        assert_eq!(scan.locals.len(), 7);
        assert!(scan.locals[6].init.ends_with('…') && scan.locals[6].init.chars().count() <= 81);
        let class = &scan.locals[1];
        assert_eq!(class.line, 9);
        assert_eq!(class.doc.as_ref().unwrap().module.as_deref(), Some("Currency"));
        assert!(scan.locals[0].doc.is_none());
        let tables: Vec<(&str, Option<usize>)> = scan.functions.iter().map(|f| (f.name.as_str(), f.local_table)).collect();
        assert_eq!(tables, vec![("Create", Some(1)), ("Init", Some(2)), ("Paint", Some(3)), ("OnRun", Some(4)), ("thing", Some(0))]);
        assert_eq!(scan.vgui_name.as_deref(), Some("cw.characterMenu"));
        assert_eq!(scan.libraries, vec![("cw.currency".to_string(), 3)]);
        assert_eq!(scan.fields, vec![("COMMAND".to_string(), "tip".to_string(), "#Commands_FallOverDesc".to_string())]);
    }

    #[test]
    fn collects_hook_calls() {
        let scan = scan(
            "--- Runs at load.
-- @param a [Number]
hook.Run('Loaded', a, 'b', { a = 1 })

function PLUGIN:PlayerSpawn(player)
  --- Lets plugins adjust the loadout.
  local loadout = Plugin.call('AdjustLoadout', player, self:GetOwner(), t[1])
  if CLIENT then
    hook.Call('Draw', GM or GAMEMODE, -x, #list, f(a, b))
  end
  timer.Simple(1, function() cw.plugin:Call('Later', player) end)
  hook.Run(name, player)
  Clockwork.plugin:Call 'NoArgs'
  my.hook.Run('NotAHook')
end

local function helper()
  plugin.Call('FromLocal')
end

Foo.bar = function()
  if !hook.Run('CanFoo', function(a, b) local c, d = 1, 2 return c end, 2) then end
end
",
        );
        let calls: Vec<(&str, Vec<&str>, Option<&str>)> = scan.hook_calls.iter().map(|c| (c.name.as_str(), c.args.iter().map(String::as_str).collect(), c.caller.as_deref())).collect();
        assert_eq!(calls, vec![
            ("Loaded", vec!["a", "'b'", "{ a = 1 }"], None),
            ("AdjustLoadout", vec!["player", "self:GetOwner()", "t[1]"], Some("PLUGIN:PlayerSpawn")),
            ("Draw", vec!["-x", "#list", "f(a, b)"], Some("PLUGIN:PlayerSpawn")),
            ("Later", vec!["player"], None),
            ("NoArgs", vec![], Some("PLUGIN:PlayerSpawn")),
            ("FromLocal", vec![], Some("helper")),
            ("CanFoo", vec!["function(a, b) local c, d = 1, 2 return c end", "2"], Some("Foo.bar")),
        ]);
        let loaded = &scan.hook_calls[0];
        assert_eq!(loaded.line, 3);
        assert_eq!(loaded.doc.as_ref().unwrap().params[0].name, "a");
        assert_eq!(scan.hook_calls[1].doc.as_ref().unwrap().summary(), "Lets plugins adjust the loadout.");
        assert_eq!(scan.hook_calls[2].realm, Realm::Client);
        assert_eq!(scan.hook_calls[1].realm, Realm::Shared);
        assert!(scan.hook_calls[2].doc.is_none());
        assert!(scan.file_doc.is_none(), "the block belongs to the hook call");
        assert_eq!(scan.functions.len(), 2, "the local function stays private");
        assert_eq!(scan.local_names, ["c", "d", "helper", "loadout"].map(String::from).into(), "locals at any depth");
    }

    #[test]
    fn collects_hook_adds() {
        let scan = scan(
            "if CLIENT then
  --- Closes the menu.
  hook.Add('VGUIMousePressed', 'cw.character:VGUIMousePressed', function(panel, code)
    function nested() end
    hook.Run('Inner')
  end)
end
hook.Add('LazyTick', self, self.rebuild)
hook.Add(name, 'x', f)
function after() end
",
        );
        assert_eq!(scan.hook_adds.len(), 2);
        let add = &scan.hook_adds[0];
        assert_eq!(add.name, "VGUIMousePressed");
        assert_eq!(add.id.as_deref(), Some("cw.character:VGUIMousePressed"));
        assert_eq!(add.params, vec!["panel", "code"]);
        assert_eq!(add.line, 3);
        assert_eq!(add.realm, Realm::Client);
        assert_eq!(add.doc.as_ref().unwrap().summary(), "Closes the menu.");
        let add = &scan.hook_adds[1];
        assert_eq!(add.id, None);
        assert!(add.params.is_empty());
        assert_eq!(add.realm, Realm::Shared);
        assert_eq!(scan.hook_calls[0].caller, None, "anonymous functions have no caller");
        assert_eq!(scan.hook_calls[0].realm, Realm::Client);
        let names: Vec<&str> = scan.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["after"]);
    }

    #[test]
    fn finds_file_docs_and_libraries() {
        let scan = scan(
            "--[[
  License.
--]]

library.New('currency', cw)
util.Include('x.lua')

--- Currencies and their exchange.
-- @module [cw.currency]

--- Not the file doc.

--- Documented.
function cw.currency:Add() end
",
        );
        let doc = scan.file_doc.as_ref().unwrap();
        assert_eq!(doc.summary(), "Currencies and their exchange.");
        assert_eq!(doc.module.as_deref(), Some("cw.currency"));
        assert_eq!(scan.functions[0].doc.as_ref().unwrap().summary(), "Documented.");

        let scan = super::scan("--- The Pipeline library.\nmod 'Pipeline'\nmod('Flux::Lang')\nlibrary.New('config', _G)\nlibrary.New('x')\n");
        assert_eq!(scan.file_doc.as_ref().unwrap().summary(), "The Pipeline library.");
        let libraries: Vec<&str> = scan.libraries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(libraries, vec!["Pipeline", "Flux.Lang", "config", "x"]);

        let scan = super::scan("---\n-- Package manager.\n\nif SERVER then end\n--- Second.\n");
        assert_eq!(scan.file_doc.as_ref().unwrap().summary(), "Package manager.");

        let scan = super::scan("--- @ignore\n\n--- @category [Hooks]\n\n--- File.\n\n--- Attached.\nlocal PANEL = {}\n");
        assert_eq!(scan.file_doc.as_ref().unwrap().summary(), "File.");
        assert_eq!(scan.categories[0].name, "Hooks");
        assert_eq!(scan.locals[0].doc.as_ref().unwrap().summary(), "Attached.");

        let scan = super::scan("local x = 1\n\n--- Too late.\n");
        assert!(scan.file_doc.is_none());
        let scan = super::scan("--- Above a statement.\nif SERVER then end\n");
        assert_eq!(scan.file_doc.as_ref().unwrap().summary(), "Above a statement.");
    }

    #[test]
    fn parses_plugin_metadata_and_fields() {
        let scan = scan(
            "PLUGIN:SetName('Stamina')
PLUGIN:SetDescription('Adds stamina.')
PLUGIN:SetAuthor('kurozael')
PLUGIN:SetGlobalAlias('cwStamina')
ENT.Type = 'anim'
ENT.PrintName = \"Item\"
ENT.Spawnable = false
SWEP.Primary.Ammo = 'none'
ITEM.name = 'Accessory' .. suffix
function ENT:Use() self.name = 'x' end
",
        );
        assert_eq!(scan.plugin_name.as_deref(), Some("Stamina"));
        assert_eq!(scan.plugin_description.as_deref(), Some("Adds stamina."));
        assert_eq!(scan.plugin_author.as_deref(), Some("kurozael"));
        assert_eq!(scan.plugin_global.as_deref(), Some("cwStamina"));
        let fields: Vec<(&str, &str, &str)> = scan.fields.iter().map(|(o, f, v)| (o.as_str(), f.as_str(), v.as_str())).collect();
        assert_eq!(fields, vec![("ENT", "Type", "anim"), ("ENT", "PrintName", "Item"), ("SWEP.Primary", "Ammo", "none")]);
        assert!(scan.spec.is_empty());
        assert_eq!(scan.functions[0].name, "Use");
    }
}
