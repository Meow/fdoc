//! Walks the tokens of one Lua file and collects the declarations that can
//! be documented, together with the documentation comment that precedes
//! each of them.

use crate::doc::{self, DocBlock};
use crate::lexer::{tokenize, Tok, Token};

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

#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub functions: Vec<FunctionDecl>,
    pub classes: Vec<ClassDecl>,
    /// `local x = FindMetaTable('Y')` style aliases: (x, Y).
    pub meta_aliases: Vec<(String, String)>,
    /// Categories declared in this file, in order.
    pub categories: Vec<Category>,
    /// `PLUGIN:set_name(...)`, `set_description`, `set_author`, `set_global`.
    pub plugin_name: Option<String>,
    pub plugin_description: Option<String>,
    pub plugin_author: Option<String>,
    pub plugin_global: Option<String>,
    /// `vgui.Register('name', PANEL)`.
    pub vgui_name: Option<String>,
    /// `s.field = 'value'` pairs inside `Package:describe`.
    pub spec: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Function,
    Other,
}

/// Returns true when the comment token can start a documentation block.
fn is_doc_start(t: &Token) -> bool {
    match &t.tok {
        Tok::Comment { text, long: false } if t.line_start => {
            // `---`, but not a separator line such as `------------`.
            text.starts_with('-') && !text.trim_end().chars().all(|c| c == '-')
        }
        _ => false,
    }
}

pub fn scan(src: &str) -> FileScan {
    let toks = tokenize(src);
    let mut out = FileScan::default();
    let mut stack: Vec<Block> = Vec::new();
    let mut category: Option<String> = None;

    // Pending doc block: raw comment texts and the line of the last one.
    let mut pending: Vec<String> = Vec::new();
    let mut pending_last_line = 0usize;

    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];

        if let Tok::Comment { text, long } = &t.tok {
            if !pending.is_empty() && !*long && t.line_start && t.line == pending_last_line + 1 {
                pending.push(text.clone());
                pending_last_line = t.line;
            } else if is_doc_start(t) {
                if !pending.is_empty() {
                    standalone(&mut out, &mut category, &pending);
                }
                pending = vec![text.clone()];
                pending_last_line = t.line;
            } else if !pending.is_empty() {
                standalone(&mut out, &mut category, &pending);
                pending.clear();
            }
            i += 1;
            continue;
        }

        // Non-comment token: the doc block attaches only if it is directly above.
        let doc_block = if !pending.is_empty() {
            let attached = t.line == pending_last_line + 1;
            let block = doc::parse(&pending);
            pending.clear();
            if attached {
                Some(block)
            } else {
                apply_standalone(&mut out, &mut category, block);
                None
            }
        } else {
            None
        };

        let in_function = stack.contains(&Block::Function);

        // Block structure.
        if let Some(name) = t.name() {
            match name {
                "function" => {
                    // `function Name(...)` declarations are handled below;
                    // the body is tracked here either way.
                    let decl = parse_function_decl(&toks, i);
                    stack.push(Block::Function);
                    if let Some((decl, next)) = decl {
                        if !in_function {
                            push_function(&mut out, decl, doc_block, &category);
                        }
                        i = next;
                        continue;
                    }
                    i += 1;
                    continue;
                }
                "if" | "do" | "repeat" => {
                    stack.push(Block::Other);
                    if let Some(block) = doc_block {
                        apply_standalone(&mut out, &mut category, block);
                    }
                    i += 1;
                    continue;
                }
                "end" | "until" => {
                    stack.pop();
                    i += 1;
                    continue;
                }
                "local" => {
                    // `local x = FindMetaTable('Y')`, `local function`, `local x = function`.
                    if let Some((alias, target)) = parse_meta_alias(&toks, i) {
                        out.meta_aliases.push((alias, target));
                    }
                    if toks.get(i + 1).is_some_and(|n| n.is_name("function")) {
                        // Local functions are private; skip the name but track the body.
                        stack.push(Block::Function);
                        i = skip_to_params_end(&toks, i + 2);
                        continue;
                    }
                    if let Some(block) = doc_block {
                        apply_standalone(&mut out, &mut category, block);
                    }
                    i += 1;
                    continue;
                }
                "class" => {
                    if let Some((decl, next)) = parse_class_decl(&toks, i, doc_block.clone()) {
                        out.classes.push(decl);
                        i = next;
                        continue;
                    }
                }
                _ => {}
            }
        }

        // `Name.path = function(...)`.
        if !in_function && let Some((decl, next)) = parse_assigned_function(&toks, i) {
            stack.push(Block::Function);
            push_function(&mut out, decl, doc_block, &category);
            i = next;
            continue;
        }

        // Metadata calls.
        if let Some(next) = parse_metadata(&toks, i, &mut out) {
            i = next;
            continue;
        }

        if let Some(block) = doc_block {
            apply_standalone(&mut out, &mut category, block);
        }
        i += 1;
    }

    if !pending.is_empty() {
        standalone(&mut out, &mut category, &pending);
    }

    out
}

fn standalone(out: &mut FileScan, category: &mut Option<String>, pending: &[String]) {
    apply_standalone(out, category, doc::parse(pending));
}

/// A doc block that is not attached to a declaration: it can switch the
/// current category. Everything else is dropped.
fn apply_standalone(out: &mut FileScan, category: &mut Option<String>, block: DocBlock) {
    if let Some(name) = block.category {
        out.categories.push(Category { name: name.clone(), description: block.description });
        *category = Some(name);
    }
}

fn push_function(out: &mut FileScan, mut decl: FunctionDecl, doc: Option<DocBlock>, category: &Option<String>) {
    if doc.as_ref().is_some_and(|d| d.ignore) {
        return;
    }
    decl.doc = doc;
    decl.category = category.clone();
    out.functions.push(decl);
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
    Some((FunctionDecl { owner, sep, name, params, line, doc: None, category: None }, next))
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
    Some((FunctionDecl { owner, sep, name, params, line, doc: None, category: None }, next))
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
fn parse_class_decl(toks: &[Token], i: usize, doc: Option<DocBlock>) -> Option<(ClassDecl, usize)> {
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
    Some((ClassDecl { name, extends, line, doc }, j))
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
            Tok::Number => "Number",
            Tok::Str(_) => "String",
            _ => return None,
        };
        return Some((alias, target.to_string()));
    }
    None
}

/// Recognises `PLUGIN:set_x('...')`, `vgui.Register('...', PANEL)` and
/// `s.field = '...'` package spec assignments. Returns the index to continue from.
fn parse_metadata(toks: &[Token], i: usize, out: &mut FileScan) -> Option<usize> {
    let t = &toks[i];
    let name = t.name()?;

    if name == "PLUGIN" && toks.get(i + 1)?.is_sym(":") {
        let method = toks.get(i + 2)?.name()?;
        if !toks.get(i + 3)?.is_sym("(") {
            return None;
        }
        let value = toks.get(i + 4)?.string()?.to_string();
        match method {
            "set_name" => out.plugin_name = Some(value),
            "set_description" => out.plugin_description = Some(value),
            "set_author" => out.plugin_author = Some(value),
            "set_global" => out.plugin_global = Some(value),
            _ => return None,
        }
        return Some(i + 5);
    }

    if name == "vgui" && toks.get(i + 1)?.is_sym(".") && toks.get(i + 2)?.is_name("Register") && toks.get(i + 3)?.is_sym("(") {
        let value = toks.get(i + 4)?.string()?.to_string();
        if out.vgui_name.is_none() {
            out.vgui_name = Some(value);
        }
        return Some(i + 5);
    }

    // `s.name = 'value'` inside `Package:describe(function(s) ... end)`.
    if t.line_start && toks.get(i + 1)?.is_sym(".") {
        let field = toks.get(i + 2)?.name()?;
        if toks.get(i + 3)?.is_sym("=")
            && let Some(value) = toks.get(i + 4)?.string()
            && matches!(field, "name" | "version" | "date" | "summary" | "description" | "author" | "website" | "license" | "global")
        {
            out.spec.push((field.to_string(), value.to_string()));
            return Some(i + 5);
        }
    }

    None
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
}
