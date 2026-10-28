//! Reads an NL file (`.nl`, `.nlua`) with NL's own parser and collects what
//! the Lua scanner collects from a Lua file (see `crate::scanner`): the
//! declarations and their doc comments, the hooks the file runs and
//! handles, realm blocks and metadata.
//!
//! A doc comment is a `##` line and the comment lines (`##` or `#`) directly
//! after it, holding the same Markdown and tags as a Lua doc comment. It
//! documents the declaration or statement that follows it. Fenced code in
//! it without a language is NL.
//!
//! What becomes what:
//!
//! - `pub fn`, `pub const` and `pub type` at the top of the file are the
//!   file's exports: the members of a module named by the path NL imports
//!   the file by (see `module_name`). Other top-level functions, constants
//!   and types are private to the file and left out, like Lua's
//!   `local function`.
//! - `struct`, `enum`, `interface` and `module` declarations are modules of
//!   their own, whether `pub` or not, with their fields, variants,
//!   members, methods (`fn Type.name`) and the methods of
//!   `interface Name for Type` implementations.
//! - `global` declarations are globals.
//! - `:Owner.name = fn ... end` defines a function on a Lua table, such as
//!   `:GM.PlayerSpawn = fn(gm, ply) ... end`; `:Owner.field = 'text'` sets
//!   a field, as in Lua.
//! - `on Name [as 'id'] do ... end`, `Hook.add` and `Event.on_run` add hook
//!   handlers; `Event.run`, `Hook.run`, `Hook.call` and the Lua paths of
//!   `scanner::HOOK_CALLERS` (`:hook.Run`) run hooks.
//! - `if SERVER`, `unless CLIENT`, their `elseif` and `else` arms and the
//!   statement modifiers `if` / `unless` decide realms and branch twins.

use std::collections::HashMap;

use nl_syntax::{SyntaxKind as K, SyntaxNode, SyntaxToken};

use crate::doc::{self, DocBlock, Param, Realm, Return};
use crate::scanner::{cut, new_decl, Branch, Category, ClassDecl, FileScan, HookAdd, HookCall, Sep, HOOK_CALLERS, MAX_CONDITION_LEN, MAX_SOURCE_LEN};

/// NL calls that run a hook by name, and how many arguments sit between the
/// hook name and the hook's own arguments (the gamemode table of
/// `Hook.call`).
const NL_HOOK_CALLERS: [(&str, usize); 3] = [("Event.run", 0), ("Hook.run", 0), ("Hook.call", 1)];

/// NL calls that add a hook handler: `(name, identifier, function)`.
const NL_HOOK_ADDERS: [&str; 2] = ["Event.on_run", "Hook.add"];

/// Lua paths that add a hook handler.
const LUA_HOOK_ADDERS: [&str; 1] = ["hook.Add"];

/// The path NL imports a file by, without its extension: below the last
/// `lua` directory, as in an addon (`addons/a/lua/a/lib.nl` is `a/lib`),
/// else below a `gamemodes` directory (`gamemodes/g/gamemode/x.nl` is
/// `g/gamemode/x`), else from the source root.
pub fn module_name(path: &str) -> String {
    let stem = path.strip_suffix(".nlua").or_else(|| path.strip_suffix(".nl")).unwrap_or(path);
    let comps: Vec<&str> = stem.split('/').collect();
    let below = |i: usize| (i + 1 < comps.len()).then(|| comps[i + 1..].join("/"));
    comps
        .iter()
        .rposition(|c| *c == "lua")
        .and_then(below)
        .or_else(|| comps.iter().position(|c| *c == "gamemodes").and_then(below))
        .unwrap_or_else(|| stem.to_string())
}

/// Scans the NL source of the file at `path`.
pub fn scan(src: &str, path: &str) -> FileScan {
    let parse = nl_syntax::parse(src);
    let root = parse.syntax();
    let mut line_starts = vec![0];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let mut s = Scanner { src, line_starts, module: module_name(path), kinds: HashMap::new(), out: FileScan::default(), category: None, seen_decl: false, exports: false, held: None };
    s.collect_kinds(&root, None);
    if let Some(block) = root.children().find(|n| n.kind() == K::BLOCK) {
        s.block(&block, &Ctx::default());
    }
    s.finish()
}

/// Where the code being visited is.
#[derive(Clone, Default)]
struct Ctx {
    /// The realm of the innermost realm-conditional block.
    realm: Option<Realm>,
    /// The innermost enclosing `if` statement and the arm it is in.
    branch: Option<Branch>,
    /// Inside a function body.
    in_fn: bool,
    /// The innermost enclosing named function, qualified as it is recorded.
    caller: Option<String>,
    /// The enclosing `module` declarations, joined with `.`.
    module: Option<String>,
}

impl Ctx {
    /// Inside the body of a function named `caller`, or of an anonymous one.
    fn function(&self, caller: Option<String>) -> Ctx {
        Ctx { in_fn: true, caller, ..self.clone() }
    }

    fn realm(&self) -> Realm {
        self.realm.unwrap_or(Realm::Shared)
    }
}

struct Scanner<'a> {
    src: &'a str,
    /// Byte offset of the start of each line.
    line_starts: Vec<usize>,
    /// The name of the module of the file's exports.
    module: String,
    /// The qualified names of the structs, enums, interfaces and modules
    /// the file declares, and the keyword of each.
    kinds: HashMap<String, &'static str>,
    out: FileScan,
    /// The category the last `@category` doc comment started.
    category: Option<String>,
    /// A declaration has been seen, so a free doc comment no longer
    /// documents the file.
    seen_decl: bool,
    /// The file exports a function, constant or type.
    exports: bool,
    /// The doc comment of the statement being visited and the line it
    /// starts on, for the hook call, hook handler or Lua path function that
    /// starts on that line.
    held: Option<(DocBlock, usize)>,
}

impl Scanner<'_> {
    fn finish(mut self) -> FileScan {
        if self.exports {
            self.out.classes.push(ClassDecl { name: self.module.clone(), extends: None, line: 1, doc: None, keyword: Some("module") });
            self.out.libraries.push((self.module.clone(), 1));
        }
        self.out
    }

    /// Records the kind of every struct, enum, interface and module below
    /// `node`, under its name qualified by the modules around it.
    fn collect_kinds(&mut self, node: &SyntaxNode, module: Option<&str>) {
        for child in node.children() {
            let keyword = match child.kind() {
                K::STRUCT_DECL => "struct",
                K::ENUM_DECL => "enum",
                K::INTERFACE_DECL => "interface",
                K::MODULE_DECL => "module",
                _ => {
                    self.collect_kinds(&child, module);
                    continue;
                }
            };
            let Some(name) = declared_name(&child) else { continue };
            let name = match module {
                Some(m) => format!("{m}.{name}"),
                None => name,
            };
            self.kinds.insert(name.clone(), keyword);
            self.collect_kinds(&child, if keyword == "module" { Some(&name) } else { module });
        }
    }

    /// `name` as it is known inside the modules of `ctx`: a type of the
    /// innermost module that declares one of this name, else `name`.
    fn qualify(&self, name: &str, ctx: &Ctx) -> String {
        let mut module = ctx.module.as_deref();
        while let Some(m) = module {
            let qualified = format!("{m}.{name}");
            if self.kinds.contains_key(&qualified) {
                return qualified;
            }
            module = m.rfind('.').map(|p| &m[..p]);
        }
        name.to_string()
    }

    /// The name a declaration in `ctx` gets: qualified by its module.
    fn in_module(name: String, ctx: &Ctx) -> String {
        match &ctx.module {
            Some(m) => format!("{m}.{name}"),
            None => name,
        }
    }

    /// The 1-based line of the first token of `node` that is not trivia.
    fn line(&self, node: &SyntaxNode) -> usize {
        let offset = first_token(node).map_or(node.text_range().start(), |t| t.text_range().start());
        self.line_starts.partition_point(|&s| s <= usize::from(offset))
    }

    /// True when only whitespace precedes the token on its line.
    fn at_line_start(&self, t: &SyntaxToken) -> bool {
        let start = usize::from(t.text_range().start());
        self.src[..start].rsplit('\n').next().is_none_or(|before| before.trim().is_empty())
    }

    /// The statements of a block, each with the doc comment directly above
    /// it; the other doc comments are free.
    fn block(&mut self, block: &SyntaxNode, ctx: &Ctx) {
        let mut run: Vec<String> = Vec::new();
        // Line breaks since the last comment line of `run`.
        let mut breaks = 0;
        for element in block.children_with_tokens() {
            if let Some(t) = element.as_token() {
                match t.kind() {
                    K::NEWLINE => breaks += 1,
                    K::WHITESPACE => {}
                    K::DOC_COMMENT | K::COMMENT if !run.is_empty() && breaks == 1 => {
                        run.push(strip_comment(t));
                        breaks = 0;
                    }
                    kind => {
                        self.flush(&mut run);
                        if kind == K::DOC_COMMENT && self.at_line_start(t) && !t.text().chars().all(|c| c == '#') {
                            run.push(strip_comment(t));
                            breaks = 0;
                        }
                    }
                }
                continue;
            }
            let Some(node) = element.as_node() else { continue };
            let doc = if !run.is_empty() && breaks <= 1 { Some(std::mem::take(&mut run)) } else { None };
            self.flush(&mut run);
            self.statement(node, ctx, doc);
            breaks = 0;
        }
        self.flush(&mut run);
    }

    /// Hands a free doc comment to `standalone`.
    fn flush(&mut self, run: &mut Vec<String>) {
        if !run.is_empty() {
            let doc = doc_block(&std::mem::take(run));
            self.standalone(doc, !self.seen_decl);
        }
    }

    /// A doc comment that documents nothing: it can start a category, or
    /// document the file when it comes before the first declaration.
    fn standalone(&mut self, doc: DocBlock, file_doc: bool) {
        if let Some(name) = doc.category {
            self.out.categories.push(Category { name: name.clone(), description: doc.description });
            self.category = Some(name);
        } else if file_doc && !doc.ignore && self.out.file_doc.is_none() {
            self.out.file_doc = Some(doc);
        }
    }

    /// Takes the held doc comment if its statement starts on `line`.
    fn take_held(&mut self, line: usize) -> Option<DocBlock> {
        if self.held.as_ref().is_some_and(|(_, l)| *l == line) { self.held.take().map(|(doc, _)| doc) } else { None }
    }

    /// A statement of a block; `run` is the free doc comment directly above
    /// it, if any.
    fn statement(&mut self, node: &SyntaxNode, ctx: &Ctx, run: Option<Vec<String>>) {
        let modified;
        let ctx = match modifier_realm(node) {
            Some(realm) => {
                modified = Ctx { realm: Some(realm), ..ctx.clone() };
                &modified
            }
            None => ctx,
        };
        let lines = attached_doc(node).or(run);
        if ctx.in_fn && node.kind() != K::FN_DECL {
            return self.expression_statement(node, ctx, lines);
        }
        match node.kind() {
            K::FN_DECL => self.fn_decl(node, ctx, lines, None),
            K::STRUCT_DECL => self.struct_decl(node, ctx, lines),
            K::ENUM_DECL => self.enum_decl(node, ctx, lines),
            K::INTERFACE_DECL => self.interface_decl(node, ctx, lines),
            K::INTERFACE_IMPL => self.interface_impl(node, ctx),
            K::MODULE_DECL => self.module_decl(node, ctx, lines),
            K::TYPE_DECL => self.type_decl(node, ctx, lines),
            K::CONST_DECL => self.const_decl(node, ctx, lines),
            K::GLOBAL_DECL => self.global_decl(node, ctx, lines),
            K::LET_DECL => {
                self.expression_statement(node, ctx, lines);
                self.seen_decl = true;
            }
            _ => self.expression_statement(node, ctx, lines),
        }
    }

    /// A statement whose doc comment, if any, documents the first hook
    /// call, hook handler or Lua path function on its first line, else is
    /// free.
    fn expression_statement(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        let outer = self.held.take();
        let line = self.line(node);
        self.held = lines.map(|l| (doc_block(&l), line));
        self.visit(node, ctx);
        if let Some((doc, _)) = self.held.take() {
            self.standalone(doc, !self.seen_decl);
        }
        self.held = outer;
    }

    /// Visits an expression or any other node for the hooks, handlers and
    /// functions in it.
    fn visit(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        match node.kind() {
            K::BLOCK => self.block(node, ctx),
            K::IF_EXPR | K::UNLESS_EXPR => self.conditional(node, ctx),
            K::FN_EXPR | K::LAMBDA_EXPR | K::TRAILING_BLOCK => self.children(node, &ctx.function(None)),
            K::ON_EXPR => self.on_expr(node, ctx),
            K::ASSIGN_STMT => self.assign(node, ctx),
            K::CALL_EXPR | K::COMMAND_CALL | K::LUA_PATH => {
                self.call(node, ctx);
                self.children(node, ctx);
            }
            _ => self.children(node, ctx),
        }
    }

    fn children(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        for child in node.children() {
            self.visit(&child, ctx);
        }
    }

    /// `if` / `unless` with their arms. A condition on `SERVER` or `CLIENT`
    /// gives its arm that realm and the arms after it the other one.
    fn conditional(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        let unless = node.kind() == K::UNLESS_EXPR;
        let if_id = usize::from(node.text_range().start());
        let mut arm: Option<Realm> = None;
        let mut rest: Option<Realm> = None;
        let mut condition = String::new();
        let arm_ctx = |realm: Option<Realm>, else_branch: bool, condition: &str| Ctx { realm: realm.or(ctx.realm), branch: Some(Branch { if_id, else_branch, condition: condition.to_string() }), ..ctx.clone() };
        for child in node.children() {
            match child.kind() {
                K::BLOCK => self.block(&child, &arm_ctx(arm, false, &condition)),
                K::ELSEIF_CLAUSE => {
                    let mut realm = None;
                    for part in child.children() {
                        if part.kind() == K::BLOCK {
                            self.block(&part, &arm_ctx(realm.or(rest), true, &condition));
                        } else {
                            realm = condition_realm(&part);
                            self.visit(&part, ctx);
                        }
                    }
                    if realm.is_some() {
                        rest = realm.map(Realm::opposite);
                    }
                }
                K::ELSE_CLAUSE => {
                    for part in child.children().filter(|p| p.kind() == K::BLOCK) {
                        self.block(&part, &arm_ctx(rest, true, &condition));
                    }
                }
                _ => {
                    let realm = condition_realm(&child);
                    arm = if unless { realm.map(Realm::opposite) } else { realm };
                    rest = arm.map(Realm::opposite);
                    let text = compact(&child, MAX_CONDITION_LEN);
                    condition = if unless { format!("not ({text})") } else { text };
                    self.visit(&child, ctx);
                }
            }
        }
    }

    // ---- Declarations ---------------------------------------------------

    /// `fn name`, `fn Type.name`, a function of a `module`, or (with
    /// `method`: its owner and category) a method of an implementation.
    fn fn_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>, method: Option<(String, String)>) {
        let name = child(node, K::NAME).or_else(|| child(node, K::OP_NAME)).map(|n| compact(&n, usize::MAX)).unwrap_or_default();
        if ctx.in_fn {
            // A local function in a function body.
            return self.children(node, &ctx.function(Some(name)));
        }
        self.seen_decl = true;
        let operator = child(node, K::OP_NAME).is_some();
        let owner = match (method, child(node, K::QUAL_NAME)) {
            (Some((owner, category)), _) => Some((owner, Some(category))),
            (None, Some(q)) => {
                let owner = self.qualify(&compact(&q, usize::MAX), ctx);
                let default = if operator {
                    "Operators"
                } else if self.kinds.get(&owner) == Some(&"module") {
                    "Functions"
                } else {
                    "Methods"
                };
                Some((owner, Some(self.category.clone().unwrap_or_else(|| default.to_string()))))
            }
            (None, None) => match &ctx.module {
                Some(m) => Some((m.clone(), self.category.clone())),
                None if has_token(node, K::PUB_KW) => {
                    self.exports = true;
                    Some((self.module.clone(), self.category.clone()))
                }
                // Private to the file.
                None => None,
            },
        };
        let Some((owner, category)) = owner else {
            return self.children(node, &ctx.function(Some(name)));
        };
        let qualified = self.push_function(node, owner, Sep::Dot, name, ctx, lines.map(|l| doc_block(&l)), category);
        self.children(node, &ctx.function(qualified));
    }

    /// Records the function whose parameters and return type are children
    /// of `node` (a declaration, an interface method, `fn` or a short
    /// function). Returns its qualified name, unless it is `@ignore`d.
    #[allow(clippy::too_many_arguments)]
    fn push_function(&mut self, node: &SyntaxNode, owner: String, sep: Sep, name: String, ctx: &Ctx, doc: Option<DocBlock>, category: Option<String>) -> Option<String> {
        let params = node.children().find(|c| matches!(c.kind(), K::PARAM_LIST | K::BLOCK_PARAMS));
        let (shown, sig) = params.as_ref().map(params_of).unwrap_or_default();
        let ret = child(node, K::RETURN_TYPE);
        let returns = ret.as_ref().map(returns_of).unwrap_or_default();
        let doc = doc.map(|d| with_signature(d, &sig, &returns));
        if doc.as_ref().is_some_and(|d| d.ignore) {
            return None;
        }
        let sep = if owner.is_empty() { Sep::None } else { sep };
        let mut decl = new_decl(owner, sep, name, shown, self.line(node));
        decl.ty = ret.as_ref().and_then(|r| r.children().next()).map(|t| compact(&t, MAX_SOURCE_LEN));
        decl.doc = doc;
        decl.category = category;
        decl.realm = ctx.realm();
        decl.branch = ctx.branch.clone();
        let qualified = decl.qualified();
        self.out.functions.push(decl);
        Some(qualified)
    }

    /// Records a value: a field, variant, constant, global or type alias.
    #[allow(clippy::too_many_arguments)]
    fn push_value(&mut self, owner: String, name: String, ty: Option<String>, init: Option<String>, category: &str, doc: Option<DocBlock>, line: usize, ctx: &Ctx) {
        if doc.as_ref().is_some_and(|d| d.ignore) {
            return;
        }
        let sep = if owner.is_empty() { Sep::None } else { Sep::Dot };
        let mut decl = new_decl(owner, sep, name, Vec::new(), line);
        decl.value = true;
        decl.ty = ty;
        decl.init = init;
        decl.doc = doc;
        decl.category = Some(category.to_string());
        decl.realm = ctx.realm();
        decl.branch = ctx.branch.clone();
        self.out.functions.push(decl);
    }

    /// A member of a struct, enum or interface: `name: Type = default`,
    /// `Variant = value`, with its own doc comment.
    fn member_value(&mut self, node: &SyntaxNode, owner: &str, category: &str, ctx: &Ctx) {
        let Some(name) = child(node, K::NAME).map(|n| compact(&n, usize::MAX)) else { return };
        let ty = after_token(node, K::COLON).map(|t| compact(&t, MAX_SOURCE_LEN));
        let init = after_token(node, K::EQ).map(|v| compact(&v, MAX_SOURCE_LEN));
        let doc = attached_doc(node).map(|l| doc_block(&l));
        self.push_value(owner.to_string(), name, ty, init, category, doc, self.line(node), ctx);
    }

    /// Records a struct, enum, interface or module. Returns its qualified
    /// name, unless it is `@ignore`d.
    fn push_class(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>, keyword: &'static str, extends: Option<String>) -> Option<String> {
        self.seen_decl = true;
        let name = Self::in_module(declared_name(node)?, ctx);
        let doc = lines.map(|l| doc_block(&l));
        if doc.as_ref().is_some_and(|d| d.ignore) {
            return None;
        }
        self.out.classes.push(ClassDecl { name: name.clone(), extends, line: self.line(node), doc, keyword: Some(keyword) });
        Some(name)
    }

    fn struct_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        let extends = child(node, K::STRUCT_PARENT).and_then(|p| p.children().next()).map(|t| type_name(&t));
        let Some(name) = self.push_class(node, ctx, lines, "struct", extends) else { return };
        for field in node.children().filter(|c| c.kind() == K::FIELD_DECL) {
            self.member_value(&field, &name, "Fields", ctx);
        }
    }

    fn enum_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        let Some(name) = self.push_class(node, ctx, lines, "enum", None) else { return };
        for variant in node.children().filter(|c| c.kind() == K::VARIANT) {
            self.member_value(&variant, &name, "Variants", ctx);
        }
    }

    fn interface_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        let Some(name) = self.push_class(node, ctx, lines, "interface", None) else { return };
        for member in node.children() {
            match member.kind() {
                K::IFACE_METHOD => {
                    let Some(method) = child(&member, K::NAME).map(|n| compact(&n, usize::MAX)) else { continue };
                    let doc = attached_doc(&member).map(|l| doc_block(&l));
                    self.push_function(&member, name.clone(), Sep::Dot, method, ctx, doc, Some("Methods".to_string()));
                }
                K::IFACE_FIELD => self.member_value(&member, &name, "Fields", ctx),
                _ => {}
            }
        }
    }

    /// `interface Name for Type`: methods of `Type`, in a category named
    /// after the interface.
    fn interface_impl(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        self.seen_decl = true;
        let mut types = node.children().filter(|c| c.kind() != K::FN_DECL && c.kind() != K::ERROR_NODE);
        let (Some(iface), Some(target)) = (types.next(), types.next()) else { return };
        let iface = type_name(&iface);
        let target = self.qualify(&type_name(&target), ctx);
        if !self.out.categories.iter().any(|c| c.name == iface) {
            self.out.categories.push(Category { name: iface.clone(), description: format!("The implementation of `{iface}`.") });
        }
        for method in node.children().filter(|c| c.kind() == K::FN_DECL) {
            let lines = attached_doc(&method);
            self.fn_decl(&method, ctx, lines, Some((target.clone(), iface.clone())));
        }
    }

    fn module_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        let Some(name) = self.push_class(node, ctx, lines, "module", None) else { return };
        let inner = Ctx { module: Some(name), ..ctx.clone() };
        for block in node.children().filter(|c| c.kind() == K::BLOCK) {
            self.block(&block, &inner);
        }
    }

    /// The owner of a constant or type declared in `ctx`: its module, the
    /// file's exports when it is `pub`, else none, as it is private.
    fn value_owner(&mut self, node: &SyntaxNode, ctx: &Ctx) -> Option<String> {
        match &ctx.module {
            Some(m) => Some(m.clone()),
            None if has_token(node, K::PUB_KW) => {
                self.exports = true;
                Some(self.module.clone())
            }
            None => None,
        }
    }

    /// `type Name = type`.
    fn type_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        self.seen_decl = true;
        let Some(owner) = self.value_owner(node, ctx) else { return };
        let Some(name) = declared_name(node) else { return };
        let init = after_token(node, K::EQ).map(|t| compact(&t, MAX_SOURCE_LEN));
        self.push_value(owner, name, None, init, "Types", lines.map(|l| doc_block(&l)), self.line(node), ctx);
    }

    /// `const a, b = x, y`; a function value is a function.
    fn const_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        self.seen_decl = true;
        let owner = self.value_owner(node, ctx);
        let values: Vec<SyntaxNode> = child(node, K::EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        let mut lines = lines;
        let patterns: Vec<SyntaxNode> = node.children().filter(|c| c.kind() != K::EXPR_LIST).collect();
        for (i, pattern) in patterns.iter().enumerate() {
            let value = values.get(i);
            let (name, ty) = binding(pattern);
            match (&owner, name) {
                (Some(owner), Some(name)) => self.named_value(owner.clone(), name, ty, value, "Constants", lines.take(), ctx, self.line(pattern)),
                _ => {
                    if let Some(v) = value {
                        self.visit(v, &ctx.function(None));
                    }
                }
            }
        }
        for extra in values.iter().skip(patterns.len()) {
            self.visit(extra, ctx);
        }
    }

    /// `global Name: Type = value`.
    fn global_decl(&mut self, node: &SyntaxNode, ctx: &Ctx, lines: Option<Vec<String>>) {
        self.seen_decl = true;
        let Some(name) = declared_name(node) else { return };
        let ty = after_token(node, K::COLON).map(|t| compact(&t, MAX_SOURCE_LEN));
        let value = after_token(node, K::EQ);
        self.named_value(String::new(), name, ty, value.as_ref(), "Variables", lines, ctx, self.line(node));
    }

    /// A constant, global or Lua field given `value`: a function when the
    /// value is a function, else a value.
    #[allow(clippy::too_many_arguments)]
    fn named_value(&mut self, owner: String, name: String, ty: Option<String>, value: Option<&SyntaxNode>, category: &str, lines: Option<Vec<String>>, ctx: &Ctx, line: usize) {
        match value.filter(|v| matches!(v.kind(), K::FN_EXPR | K::LAMBDA_EXPR)) {
            Some(f) => {
                let qualified = self.push_function(f, owner, Sep::Dot, name, ctx, lines.map(|l| doc_block(&l)), self.category.clone());
                self.children(f, &ctx.function(qualified));
            }
            None => {
                let init = value.map(|v| compact(v, MAX_SOURCE_LEN));
                self.push_value(owner, name, ty, init, category, lines.map(|l| doc_block(&l)), line, ctx);
                if let Some(v) = value {
                    self.visit(v, ctx);
                }
            }
        }
    }

    /// `:Owner.name = fn ... end` defines a function on a Lua table, and
    /// `:Owner.field = 'text'` sets a field, outside of functions.
    fn assign(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        let mut parts = node.children();
        let (Some(target), Some(value)) = (parts.next(), parts.next()) else { return self.children(node, ctx) };
        let path = (target.kind() == K::LUA_PATH && !ctx.in_fn).then(|| lua_fields(&target)).flatten();
        let Some(mut path) = path else { return self.children(node, ctx) };
        if path.len() > 1 && path[0] == "_G" {
            path.remove(0);
        }
        let Some(name) = path.pop() else { return self.children(node, ctx) };
        let owner = path.join(".");
        if matches!(value.kind(), K::FN_EXPR | K::LAMBDA_EXPR) {
            self.seen_decl = true;
            let line = self.line(node);
            let doc = self.take_held(line);
            let qualified = self.push_function(&value, owner, Sep::Dot, name, ctx, doc, self.category.clone());
            return self.children(&value, &ctx.function(qualified));
        }
        if !owner.is_empty()
            && let Some(text) = string_literal(&value)
        {
            self.out.fields.push((owner, name, text));
        }
        self.visit(&value, ctx);
    }

    /// `on Name [as 'id'] do |params| ... end`.
    fn on_expr(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        let line = self.line(node);
        let doc = self.take_held(line);
        if let Some(name) = child(node, K::QUAL_NAME).map(|q| compact(&q, usize::MAX)) {
            let id = node.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == K::STRING).and_then(|t| decode(t.text()));
            let (params, sig) = child(node, K::BLOCK_PARAMS).as_ref().map(params_of).unwrap_or_default();
            let doc = doc.map(|d| with_signature(d, &sig, &[]));
            if !doc.as_ref().is_some_and(|d| d.ignore) {
                self.out.hook_adds.push(HookAdd { name, id, params, line, realm: ctx.realm(), doc });
            }
        }
        self.children(node, &ctx.function(None));
    }

    /// A call that runs a hook, adds a handler or sets plugin metadata.
    fn call(&mut self, node: &SyntaxNode, ctx: &Ctx) {
        let Some((path, args)) = callee(node) else { return };
        let line = self.line(node);
        let lua = path.strip_prefix(':');
        let skip = match lua {
            Some(p) => HOOK_CALLERS.iter().find(|(c, _)| *c == p).map(|(_, s)| *s),
            None => NL_HOOK_CALLERS.iter().find(|(c, _)| *c == path).map(|(_, s)| *s),
        };
        if let Some(skip) = skip {
            let Some(name) = args.first().and_then(string_literal) else { return };
            let args = args.iter().skip(1 + skip).map(|a| compact(a, MAX_SOURCE_LEN)).collect();
            let doc = self.take_held(line);
            self.out.hook_calls.push(HookCall { name, args, line, realm: ctx.realm(), doc, caller: ctx.caller.clone() });
            return;
        }
        let adds = match lua {
            Some(p) => LUA_HOOK_ADDERS.contains(&p),
            None => NL_HOOK_ADDERS.contains(&path.as_str()),
        };
        if adds {
            let Some(name) = args.first().and_then(string_literal) else { return };
            let id = args.get(1).and_then(string_literal);
            let handler = args.get(2).filter(|f| matches!(f.kind(), K::FN_EXPR | K::LAMBDA_EXPR | K::TRAILING_BLOCK));
            let (params, sig) = handler.and_then(|f| f.children().find(|c| matches!(c.kind(), K::PARAM_LIST | K::BLOCK_PARAMS))).as_ref().map(params_of).unwrap_or_default();
            let doc = self.take_held(line).map(|d| with_signature(d, &sig, &[]));
            if !doc.as_ref().is_some_and(|d| d.ignore) {
                self.out.hook_adds.push(HookAdd { name, id, params, line, realm: ctx.realm(), doc });
            }
            return;
        }
        let Some(value) = args.first().and_then(string_literal) else { return };
        match lua {
            Some("PLUGIN:set_name" | "PLUGIN:SetName") => self.out.plugin_name = Some(value),
            Some("PLUGIN:set_description" | "PLUGIN:SetDescription") => self.out.plugin_description = Some(value),
            Some("PLUGIN:set_author" | "PLUGIN:SetAuthor") => self.out.plugin_author = Some(value),
            Some("PLUGIN:set_global" | "PLUGIN:SetGlobal" | "PLUGIN:SetGlobalAlias") => self.out.plugin_global = Some(value),
            Some("vgui.Register") => {
                self.out.vgui_name.get_or_insert(value);
            }
            _ => {}
        }
    }
}

// ---- Tree helpers -------------------------------------------------------

/// The first child node of `kind`.
fn child(node: &SyntaxNode, kind: K) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

/// True when `node` has a token of `kind` among its children.
fn has_token(node: &SyntaxNode, kind: K) -> bool {
    node.children_with_tokens().any(|e| e.as_token().is_some_and(|t| t.kind() == kind))
}

/// The first child node after the first child token of `kind`.
fn after_token(node: &SyntaxNode, kind: K) -> Option<SyntaxNode> {
    let mut seen = false;
    for e in node.children_with_tokens() {
        match e.as_node() {
            Some(n) if seen => return Some(n.clone()),
            Some(_) => {}
            None => seen |= e.as_token().is_some_and(|t| t.kind() == kind),
        }
    }
    None
}

/// The first token below `node` that is not trivia.
fn first_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens().filter_map(|e| e.into_token()).find(|t| !t.kind().is_trivia())
}

/// The name a declaration declares: its `NAME` or `QUAL_NAME` child.
fn declared_name(node: &SyntaxNode) -> Option<String> {
    node.children().find(|c| matches!(c.kind(), K::NAME | K::QUAL_NAME)).map(|n| compact(&n, usize::MAX))
}

/// The name of a type without its type arguments: `Point` for `Point<T>`.
fn type_name(ty: &SyntaxNode) -> String {
    match child(ty, K::QUAL_NAME) {
        Some(q) => compact(&q, usize::MAX),
        None => compact(ty, MAX_SOURCE_LEN),
    }
}

/// The source of `node` on one line: comments left out, each run of
/// whitespace and line breaks a single space. Cut after `max` characters.
fn compact(node: &SyntaxNode, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    let mut chars = 0;
    for t in node.descendants_with_tokens().filter_map(|e| e.into_token()) {
        if t.kind().is_trivia() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
            chars += 1;
        }
        space = false;
        out.push_str(t.text());
        chars += t.text().chars().count();
        if chars > max {
            break;
        }
    }
    cut(out, max)
}

/// The text of a comment line without its `##` or `#` and one space.
fn strip_comment(t: &SyntaxToken) -> String {
    let text = t.text();
    let text = text.strip_prefix("##").or_else(|| text.strip_prefix('#')).unwrap_or(text);
    text.strip_prefix(' ').unwrap_or(text).to_string()
}

/// The doc comment the parser put inside `node`, or inside the node it
/// starts with (the `on` of an expression statement), as lines.
fn attached_doc(node: &SyntaxNode) -> Option<Vec<String>> {
    let mut n = node.clone();
    loop {
        if let Some(text) = nl_syntax::doc_comment(&n) {
            return Some(text.lines().map(str::to_string).collect());
        }
        let first = n.children_with_tokens().find(|e| !e.as_token().is_some_and(|t| t.kind().is_trivia()))?;
        n = first.into_node()?;
    }
}

/// Parses doc comment lines. A fenced code block without a language is NL.
fn doc_block(lines: &[String]) -> DocBlock {
    let mut out = Vec::with_capacity(lines.len());
    let mut open = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if !open && trimmed.trim_end() == "```" {
                out.push(format!("{}```nl", &line[..line.len() - trimmed.len()]));
                open = true;
                continue;
            }
            open = !open;
        }
        out.push(line.trim_end().to_string());
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    doc::parse_lines(&out)
}

/// Fills in what the signature says and the doc comment does not: every
/// parameter, with its type and default, when the comment has no `@param`
/// (and no `@variant`), else the types and defaults of the parameters it
/// names; the return types when it has no `@return`, else those its
/// `@return` values leave out. A tag's first word is only a type when it
/// looks like an NL type, so `@return [the count]` is a description.
fn with_signature(mut doc: DocBlock, sig: &[Param], returns: &[Return]) -> DocBlock {
    let bare = |n: &str| n.trim_start_matches('*').to_string();
    if doc.params.is_empty() && doc.variants.is_empty() {
        doc.params = sig.to_vec();
    } else {
        for p in &mut doc.params {
            let Some(s) = sig.iter().find(|s| bare(&s.name) == bare(&p.name)) else { continue };
            if s.ty.is_some() {
                untype(&mut p.ty, &mut p.description);
            }
            p.ty = p.ty.take().or_else(|| s.ty.clone());
            p.default = p.default.take().or_else(|| s.default.clone());
        }
    }
    if doc.returns.is_empty() && doc.variants.is_empty() {
        doc.returns = returns.to_vec();
    } else if doc.returns.len() == returns.len() {
        for (r, s) in doc.returns.iter_mut().zip(returns) {
            untype(&mut r.ty, &mut r.description);
            r.ty = r.ty.take().or_else(|| s.ty.clone());
        }
    }
    doc
}

/// Moves a tag's "type" that is a word of its description back into it.
fn untype(ty: &mut Option<String>, description: &mut String) {
    if let Some(word) = ty.take_if(|t| !looks_like_type(t)) {
        *description = if description.is_empty() { word } else { format!("{word} {description}") };
    }
}

/// True for what can start an NL type: a capitalised name, a list,
/// record, tuple or function type, or one of the lower-case type names.
fn looks_like_type(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_uppercase() || matches!(c, '[' | '{' | '(')) || matches!(word.trim_end_matches('?'), "nil" | "any" | "unknown" | "never")
}

/// The parameters of a `PARAM_LIST` or `BLOCK_PARAMS`: as they are shown
/// in the signature (`amount: Number`, `n = 1`, `*rest`), and as doc
/// parameters with their types and defaults.
fn params_of(list: &SyntaxNode) -> (Vec<String>, Vec<Param>) {
    let mut shown = Vec::new();
    let mut params = Vec::new();
    for p in list.children() {
        let prefix = match p.kind() {
            K::PARAM => "",
            K::REST_PARAM => "*",
            K::KEYED_PARAM => "**",
            _ => continue,
        };
        shown.push(compact(&p, MAX_SOURCE_LEN));
        let (name, ty, default) = match p.kind() {
            K::PARAM => {
                let pattern = p.children().next();
                let (name, ty) = pattern.as_ref().map(binding).unwrap_or_default();
                let name = name.or_else(|| pattern.map(|n| compact(&n, MAX_SOURCE_LEN))).unwrap_or_default();
                (name, ty, after_token(&p, K::EQ).map(|d| compact(&d, MAX_SOURCE_LEN)))
            }
            _ => {
                let name = child(&p, K::NAME).map(|n| compact(&n, usize::MAX)).unwrap_or_default();
                (format!("{prefix}{name}"), after_token(&p, K::COLON).map(|t| compact(&t, MAX_SOURCE_LEN)), None)
            }
        };
        params.push(Param { name, default, ty, description: String::new() });
    }
    (shown, params)
}

/// The name and type a pattern binds, for a plain or typed name.
fn binding(pattern: &SyntaxNode) -> (Option<String>, Option<String>) {
    match pattern.kind() {
        K::BIND_PAT => (child(pattern, K::NAME).map(|n| compact(&n, usize::MAX)), None),
        K::TYPED_PAT => {
            let name = pattern.children().next().filter(|n| n.kind() == K::BIND_PAT).and_then(|b| child(&b, K::NAME)).map(|n| compact(&n, usize::MAX));
            (name, after_token(pattern, K::COLON).map(|t| compact(&t, MAX_SOURCE_LEN)))
        }
        _ => (None, None),
    }
}

/// The return values a `RETURN_TYPE` declares: one per type of a tuple,
/// none for `nil`.
fn returns_of(ret: &SyntaxNode) -> Vec<Return> {
    let Some(ty) = ret.children().next() else { return Vec::new() };
    let types: Vec<SyntaxNode> = if ty.kind() == K::TUPLE_TYPE { ty.children().collect() } else { vec![ty] };
    types.iter().map(|t| compact(t, MAX_SOURCE_LEN)).filter(|t| t != "nil").map(|t| Return { ty: Some(t), description: String::new() }).collect()
}

/// The realm a condition checks for: `SERVER` or `CLIENT`, flipped by a
/// `not` or `!` directly before it. None when it names neither or both.
fn condition_realm(cond: &SyntaxNode) -> Option<Realm> {
    let toks: Vec<SyntaxToken> = cond.descendants_with_tokens().filter_map(|e| e.into_token()).filter(|t| !t.kind().is_trivia()).collect();
    let mut found = None;
    for (i, t) in toks.iter().enumerate() {
        let realm = match (t.kind(), t.text()) {
            (K::CNAME, "SERVER") => Realm::Server,
            (K::CNAME, "CLIENT") => Realm::Client,
            _ => continue,
        };
        let prev = i.checked_sub(1).and_then(|j| toks.get(j)).map(|p| p.kind());
        if matches!(prev, Some(K::DOT | K::QUESTION_DOT)) {
            continue;
        }
        let realm = if matches!(prev, Some(K::NOT_KW | K::BANG)) { realm.opposite() } else { realm };
        match found {
            Some(r) if r != realm => return None,
            _ => found = Some(realm),
        }
    }
    found
}

/// The realm a statement modifier (`x if SERVER`, `x unless CLIENT`) runs
/// the statement in.
fn modifier_realm(stmt: &SyntaxNode) -> Option<Realm> {
    let modifier = child(stmt, K::MODIFIER)?;
    let cond = modifier.children().next()?;
    let realm = condition_realm(&cond)?;
    if has_token(&modifier, K::UNLESS_KW) {
        Some(realm.opposite())
    } else if has_token(&modifier, K::IF_KW) {
        Some(realm)
    } else {
        None
    }
}

/// The value of a string literal without interpolation.
fn string_literal(node: &SyntaxNode) -> Option<String> {
    if node.kind() != K::LITERAL {
        return None;
    }
    let t = node.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == K::STRING)?;
    decode(t.text())
}

/// The value of a `STRING` token.
fn decode(text: &str) -> Option<String> {
    let parts = nl_syntax::decode_string(&[text]);
    parts.into_iter().next().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The names of a Lua path made of fields only (`:GM.Name`), else None.
fn lua_fields(path: &SyntaxNode) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for e in path.children_with_tokens() {
        match e.as_node() {
            Some(step) if step.kind() == K::LUA_FIELD => parts.push(step.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind().is_name() || t.kind().is_keyword())?.text().to_string()),
            Some(_) => return None,
            None => {
                let t = e.as_token()?;
                if t.kind().is_name() || t.kind().is_keyword() {
                    parts.push(t.text().to_string());
                }
            }
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// The callee of a call as a path (`Event.run`, or `:hook.Run` for a Lua
/// path) and its arguments, a trailing block last.
fn callee(node: &SyntaxNode) -> Option<(String, Vec<SyntaxNode>)> {
    match node.kind() {
        K::CALL_EXPR | K::COMMAND_CALL => {
            let mut children = node.children();
            let path = dotted(&children.next()?)?;
            let mut args = Vec::new();
            for c in children {
                match c.kind() {
                    K::ARG_LIST | K::COMMAND_ARGS => args.extend(c.children()),
                    K::TRAILING_BLOCK => args.push(c),
                    _ => {}
                }
            }
            Some((path, args))
        }
        K::LUA_PATH => {
            let mut path = String::from(":");
            for e in node.children_with_tokens() {
                let Some(step) = e.as_node() else {
                    let t = e.as_token()?;
                    if t.kind().is_name() || t.kind().is_keyword() {
                        path.push_str(t.text());
                    }
                    continue;
                };
                let name = || step.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind().is_name() || t.kind().is_keyword()).map(|t| t.text().to_string());
                match step.kind() {
                    K::LUA_FIELD => {
                        path.push('.');
                        path.push_str(&name()?);
                    }
                    K::LUA_METHOD | K::LUA_CALL => {
                        if step.kind() == K::LUA_METHOD {
                            path.push(':');
                            path.push_str(&name()?);
                        }
                        let mut args = Vec::new();
                        for c in step.children() {
                            match c.kind() {
                                K::ARG_LIST => args.extend(c.children()),
                                K::TRAILING_BLOCK => args.push(c),
                                _ => {}
                            }
                        }
                        return Some((path, args));
                    }
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

/// A callee as a dotted path: `Event.run` for `Event.run`, `:hook.Run` for
/// a Lua path without calls.
fn dotted(node: &SyntaxNode) -> Option<String> {
    match node.kind() {
        K::NAME_REF => Some(compact(node, usize::MAX)),
        K::FIELD_EXPR => {
            let base = dotted(&node.children().next()?)?;
            let field = node.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind().is_name() || t.kind().is_keyword()).last()?;
            Some(format!("{base}.{}", field.text()))
        }
        K::LUA_PATH => lua_fields(node).map(|parts| format!(":{}", parts.join("."))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_nl(src: &str) -> FileScan {
        scan(src, "addons/x/lua/x/sh_lib.nl")
    }

    #[test]
    fn names_modules_by_their_import_path() {
        assert_eq!(module_name("addons/nltest/lua/nltest/lib.nl"), "nltest/lib");
        assert_eq!(module_name("gamemodes/nlgm/gamemode/nlgm_lib.nl"), "nlgm/gamemode/nlgm_lib");
        assert_eq!(module_name("lib/util.nlua"), "lib/util");
        assert_eq!(module_name("lua/a.nl"), "a");
    }

    #[test]
    fn documents_exports_and_skips_private_functions() {
        let s = scan_nl("## The library.\n\nimport { x } from './y'\n\n## Greets.\npub fn greet(who: String, n: Number = 1): String\n  \"hi\"\nend\n\n## Private.\nfn helper end\n\npub const limit = 10\n");
        assert_eq!(s.file_doc.as_ref().map(|d| d.summary()), Some("The library.".to_string()));
        let names: Vec<String> = s.functions.iter().map(|f| f.qualified()).collect();
        assert_eq!(names, vec!["x/sh_lib.greet", "x/sh_lib.limit"]);
        let greet = &s.functions[0];
        assert_eq!(greet.params, vec!["who: String", "n: Number = 1"]);
        assert_eq!(greet.ty.as_deref(), Some("String"));
        let doc = greet.doc.as_ref().unwrap();
        assert_eq!(doc.params.iter().map(|p| (p.name.as_str(), p.ty.as_deref(), p.default.as_deref())).collect::<Vec<_>>(), vec![("who", Some("String"), None), ("n", Some("Number"), Some("1"))]);
        assert_eq!(doc.returns[0].ty.as_deref(), Some("String"));
        let limit = &s.functions[1];
        assert!(limit.value);
        assert_eq!((limit.init.as_deref(), limit.category.as_deref()), (Some("10"), Some("Constants")));
        assert_eq!(s.classes.iter().map(|c| (c.name.as_str(), c.keyword)).collect::<Vec<_>>(), vec![("x/sh_lib", Some("module"))]);
        assert_eq!(s.libraries, vec![("x/sh_lib".to_string(), 1)]);
    }

    #[test]
    fn documents_structs_fields_and_methods() {
        let s = scan_nl("## A player.\nstruct Player : Base\n  ## The name.\n  name: String\n  health: Number = 100\nend\n\n## Takes damage.\n## @param amount The damage\n## @return [the health left, Bool whether it died]\nfn Player.damage(amount: Number): (Number, Bool)\nend\n\nfn Player.+(other) end\n");
        assert_eq!((s.classes[0].name.as_str(), s.classes[0].extends.as_deref(), s.classes[0].keyword), ("Player", Some("Base"), Some("struct")));
        assert_eq!(s.classes[0].doc.as_ref().unwrap().summary(), "A player.");
        let f = s.functions.iter().map(|f| (f.name.as_str(), f.value, f.ty.as_deref(), f.init.as_deref(), f.category.as_deref())).collect::<Vec<_>>();
        assert_eq!(f, vec![("name", true, Some("String"), None, Some("Fields")), ("health", true, Some("Number"), Some("100"), Some("Fields")), ("damage", false, Some("(Number, Bool)"), None, Some("Methods")), ("+", false, None, None, Some("Operators"))]);
        assert_eq!(s.functions[0].doc.as_ref().unwrap().summary(), "The name.");
        let damage = s.functions[2].doc.as_ref().unwrap();
        assert_eq!((damage.params[0].ty.as_deref(), damage.params[0].description.as_str()), (Some("Number"), "The damage"));
        assert_eq!(damage.returns.iter().map(|r| (r.ty.as_deref().unwrap(), r.description.as_str())).collect::<Vec<_>>(), vec![("Number", "the health left"), ("Bool", "whether it died")]);
    }

    #[test]
    fn documents_enums_interfaces_modules_and_globals() {
        let s = scan_nl("enum Mode\n  Easy\n  Hard = 2\nend\ninterface Named\n  ## Its name.\n  fn name_of: String\n  size: Number\nend\ninterface Named for Mode\n  fn name_of: String\n    \"m\"\n  end\nend\nmodule Inv\n  struct Slot\n  end\n  fn add(item) end\n  fn Slot.take end\n  const max = 4\nend\n## The score.\nglobal Score: Number = 10\nglobal shout = fn(s) s end\n");
        let classes: Vec<(&str, Option<&str>)> = s.classes.iter().map(|c| (c.name.as_str(), c.keyword)).collect();
        assert_eq!(classes, vec![("Mode", Some("enum")), ("Named", Some("interface")), ("Inv", Some("module")), ("Inv.Slot", Some("struct"))]);
        let f: Vec<(String, bool, Option<&str>)> = s.functions.iter().map(|f| (f.qualified(), f.value, f.category.as_deref())).collect();
        assert_eq!(
            f,
            vec![
                ("Mode.Easy".to_string(), true, Some("Variants")),
                ("Mode.Hard".to_string(), true, Some("Variants")),
                ("Named.name_of".to_string(), false, Some("Methods")),
                ("Named.size".to_string(), true, Some("Fields")),
                ("Mode.name_of".to_string(), false, Some("Named")),
                ("Inv.add".to_string(), false, None),
                ("Inv.Slot.take".to_string(), false, Some("Methods")),
                ("Inv.max".to_string(), true, Some("Constants")),
                ("Score".to_string(), true, Some("Variables")),
                ("shout".to_string(), false, None),
            ]
        );
        assert_eq!(s.functions[2].doc.as_ref().unwrap().summary(), "Its name.");
        assert_eq!(s.functions[8].doc.as_ref().unwrap().summary(), "The score.");
        assert_eq!(s.categories[0].name, "Named");
    }

    #[test]
    fn reads_hooks_handlers_and_lua_paths() {
        let s = scan_nl("## Adds the hook.\n:GM.PlayerSpawned = fn(gm, ply: Player) end\n:PLUGIN.name = 'Admin'\n:PLUGIN:set_author('Me')\n\n## Hands out items.\non PlayerSpawn as 'items.spawn' do |ply: Player|\n  ## Gives items.\n  Event.run \"GiveItems\", ply, 2\nend\n\npub fn tick\n  :hook.Run(\"Ticked\", 1)\n  Hook.call \"Called\", nil, 3\nend\nHook.add(\"Think\", \"x\", fn(dt) end)\n");
        let f = &s.functions[0];
        assert_eq!((f.qualified().as_str(), f.params.clone()), ("GM.PlayerSpawned", vec!["gm".to_string(), "ply: Player".to_string()]));
        assert_eq!(f.doc.as_ref().unwrap().summary(), "Adds the hook.");
        assert_eq!(s.fields, vec![("PLUGIN".to_string(), "name".to_string(), "Admin".to_string())]);
        assert_eq!(s.plugin_author.as_deref(), Some("Me"));
        let adds = s.hook_adds.iter().map(|a| (a.name.as_str(), a.id.as_deref(), a.params.clone(), a.doc.as_ref().map(|d| d.summary()))).collect::<Vec<_>>();
        assert_eq!(adds, vec![("PlayerSpawn", Some("items.spawn"), vec!["ply: Player".to_string()], Some("Hands out items.".to_string())), ("Think", Some("x"), vec!["dt".to_string()], None)]);
        let calls = s.hook_calls.iter().map(|c| (c.name.as_str(), c.args.clone(), c.caller.as_deref(), c.doc.as_ref().map(|d| d.summary()))).collect::<Vec<_>>();
        assert_eq!(
            calls,
            vec![
                ("GiveItems", vec!["ply".to_string(), "2".to_string()], None, Some("Gives items.".to_string())),
                ("Ticked", vec!["1".to_string()], Some("x/sh_lib.tick"), None),
                ("Called", vec!["3".to_string()], Some("x/sh_lib.tick"), None),
            ]
        );
        assert!(s.file_doc.is_none(), "a doc comment above a declaration is not the file's");
    }

    #[test]
    fn reads_realms_and_branches() {
        let s = scan_nl("if SERVER\n  fn A.x end\nelse\n  fn A.x end\nend\nunless CLIENT\n  fn A.y end\nend\nif not SERVER\n  Event.run 'C'\nend\nEvent.run 'S' if SERVER\nif debug\n  fn A.z end\nelseif CLIENT\n  fn A.z end\nelse\n  fn A.z end\nend\n");
        let realms: Vec<(&str, Realm, Option<bool>)> = s.functions.iter().map(|f| (f.name.as_str(), f.realm, f.branch.as_ref().map(|b| b.else_branch))).collect();
        assert_eq!(
            realms,
            vec![("x", Realm::Server, Some(false)), ("x", Realm::Client, Some(true)), ("y", Realm::Server, Some(false)), ("z", Realm::Shared, Some(false)), ("z", Realm::Client, Some(true)), ("z", Realm::Server, Some(true))]
        );
        assert_eq!(s.functions[0].branch.as_ref().unwrap().condition, "SERVER");
        assert_eq!(s.functions[2].branch.as_ref().unwrap().condition, "not (CLIENT)");
        let calls: Vec<(&str, Realm)> = s.hook_calls.iter().map(|c| (c.name.as_str(), c.realm)).collect();
        assert_eq!(calls, vec![("C", Realm::Client), ("S", Realm::Server)]);
    }

    #[test]
    fn reads_categories_ignore_and_nl_fences() {
        let s = scan_nl("## @category [Spawning] Spawning things.\n\n## Spawns.\n##\n## ```\n## spawn 1\n## ```\nfn A.spawn end\n## @ignore\nfn A.hidden end\n");
        assert_eq!(s.categories[0].name, "Spawning");
        assert_eq!(s.functions.len(), 1);
        assert_eq!(s.functions[0].category.as_deref(), Some("Spawning"));
        assert!(s.functions[0].doc.as_ref().unwrap().description.contains("```nl\nspawn 1\n```"), "{:?}", s.functions[0].doc);
        assert!(s.file_doc.is_none());
    }

    #[test]
    fn hook_docs_need_the_same_line() {
        let s = scan_nl("## Not the hook's.\nif SERVER\n  Event.run 'A'\nend\n");
        assert!(s.hook_calls[0].doc.is_none());
        assert_eq!(s.file_doc.as_ref().map(|d| d.summary()), Some("Not the hook's.".to_string()));
    }
}
