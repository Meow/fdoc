//! Writes the documentation model as one JSON document (`--format json`),
//! for tools that read the documentation rather than browse it. The shape
//! is described in the README.

use std::fmt::Write;

use crate::doc::{DocBlock, Param, Realm, Return};
use crate::model::{Function, FunctionKind, Group, Module, ModuleKind, Project, Section};

/// The version of the document's shape, raised when a field is renamed,
/// removed or changes its meaning.
pub const FORMAT_VERSION: i64 = 1;

/// A JSON value. The members of an object keep their order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<usize> for Value {
    fn from(n: usize) -> Value {
        Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Str(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(s)
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Value {
        Value::Str(s.clone())
    }
}

impl From<Realm> for Value {
    fn from(realm: Realm) -> Value {
        Value::from(realm_name(realm))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map_or(Value::Null, Into::into)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Value {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

/// An object with the members in the order given.
fn object<const N: usize>(members: [(&str, Value); N]) -> Value {
    Value::Object(members.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// An array of the values `f` makes of `items`.
fn array<T>(items: impl IntoIterator<Item = T>, f: impl Fn(T) -> Value) -> Value {
    Value::Array(items.into_iter().map(f).collect())
}

fn realm_name(realm: Realm) -> &'static str {
    match realm {
        Realm::Server => "server",
        Realm::Client => "client",
        Realm::Shared => "shared",
    }
}

/// Writes `s` as a JSON string literal. Everything but printable ASCII is
/// escaped, other characters as `\uXXXX` (UTF-16, so a character outside
/// the Basic Multilingual Plane is a surrogate pair), and the output is
/// plain ASCII.
pub fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// True for a value written on one line: anything but a non-empty array or
/// object.
fn is_flat(v: &Value) -> bool {
    match v {
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        _ => true,
    }
}

/// Writes `v` indented by two spaces per level, starting at `level`.
/// Arrays of plain values stay on one line.
fn write_value(v: &Value, level: usize, out: &mut String) {
    let pad = |out: &mut String, level: usize| out.extend(std::iter::repeat_n("  ", level));
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Str(s) => escape(s, out),
        Value::Array(items) if items.iter().all(is_flat) => {
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                write_value(item, level, out);
            }
            out.push(']');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (k, item) in items.iter().enumerate() {
                pad(out, level + 1);
                write_value(item, level + 1, out);
                out.push_str(if k + 1 < items.len() { ",\n" } else { "\n" });
            }
            pad(out, level);
            out.push(']');
        }
        Value::Object(members) if members.is_empty() => out.push_str("{}"),
        Value::Object(members) => {
            out.push_str("{\n");
            for (k, (key, value)) in members.iter().enumerate() {
                pad(out, level + 1);
                escape(key, out);
                out.push_str(": ");
                write_value(value, level + 1, out);
                out.push_str(if k + 1 < members.len() { ",\n" } else { "\n" });
            }
            pad(out, level);
            out.push('}');
        }
    }
}

/// The project as a JSON document, with a final line break.
pub fn render(project: &Project) -> String {
    let mut out = String::new();
    write_value(&project_value(project), 0, &mut out);
    out.push('\n');
    out
}

fn project_value(p: &Project) -> Value {
    let mut index: Vec<(&String, &String)> = p.index.iter().collect();
    index.sort();
    object([
        ("title", p.title.as_str().into()),
        ("version", p.version.as_ref().into()),
        ("summary", p.summary.as_ref().into()),
        ("description", p.description.as_ref().into()),
        ("source_url", p.source_url.as_ref().into()),
        ("format_version", Value::Int(FORMAT_VERSION)),
        ("generator", format!("fdoc {}", env!("CARGO_PKG_VERSION")).into()),
        ("sections", array(&p.sections, section_value)),
        ("index", Value::Object(index.into_iter().map(|(k, v)| (k.clone(), v.into())).collect())),
    ])
}

fn section_value(s: &Section) -> Value {
    object([("title", s.title.as_str().into()), ("groups", array(&s.groups, group_value))])
}

fn group_value(g: &Group) -> Value {
    object([
        ("title", g.title.as_str().into()),
        ("path", g.dir.as_str().into()),
        ("source_dir", g.source_dir.as_str().into()),
        ("core", g.is_core.into()),
        ("description", g.description.as_ref().into()),
        ("author", g.author.as_ref().into()),
        ("version", g.version.as_ref().into()),
        ("realm", g.realm.into()),
        ("modules", array(&g.modules, module_value)),
    ])
}

fn module_value(m: &Module) -> Value {
    let (module_kind, keyword, extends, object_name) = match &m.kind {
        ModuleKind::Class { extends, keyword } => ("class", *keyword, extends.as_ref(), None),
        ModuleKind::Library => ("library", None, None, None),
        ModuleKind::Hooks => ("hooks", None, None, None),
        ModuleKind::Handlers { object } => ("handlers", None, None, Some(object)),
        ModuleKind::Globals => ("globals", None, None, None),
        ModuleKind::Object { object } => ("object", None, None, Some(object)),
        ModuleKind::Template => ("template", None, None, None),
    };
    object([
        ("id", m.id.as_str().into()),
        ("title", m.title.as_str().into()),
        ("subtitle", m.subtitle.as_ref().into()),
        ("kind", m.badge().into()),
        ("module_kind", module_kind.into()),
        ("keyword", keyword.into()),
        ("extends", extends.into()),
        ("object", object_name.into()),
        ("definition", m.definition.as_ref().into()),
        ("realm", m.realm.into()),
        ("summary", m.summary().into()),
        ("description", m.doc.as_ref().map(|d| d.description.as_str()).unwrap_or_default().into()),
        ("doc", m.doc.as_ref().map_or(Value::Null, doc_value)),
        ("files", m.files.iter().collect::<Vec<_>>().into()),
        ("declared_in", m.decl_file.as_ref().map_or(Value::Null, |(file, line)| location(file, *line))),
        ("slug", m.slug.as_str().into()),
        ("categories", array(&m.categories, |c| object([("name", c.name.as_str().into()), ("description", c.description.as_str().into())]))),
        ("functions", array(&m.functions, function_value)),
    ])
}

fn location(file: &str, line: usize) -> Value {
    object([("file", file.into()), ("line", line.into())])
}

fn function_value(f: &Function) -> Value {
    let kind = match f.kind {
        FunctionKind::Declared => "declared",
        FunctionKind::Hook => "hook",
        FunctionKind::HookAdd => "hook_add",
        FunctionKind::Value => "value",
    };
    object([
        ("name", f.name.as_str().into()),
        ("owner", f.owner.as_str().into()),
        ("qualified", f.qualified().into()),
        ("signature", f.qualified_signature().into()),
        ("params", f.params.iter().collect::<Vec<_>>().into()),
        ("kind", kind.into()),
        ("realm", f.realm.into()),
        ("file", f.file.as_str().into()),
        ("line", f.line.into()),
        ("sources", array(&f.sources, |(file, line, realm)| object([("file", file.into()), ("line", (*line).into()), ("realm", (*realm).into())]))),
        ("summary", f.summary().into()),
        ("description", f.doc.as_ref().map(|d| d.description.as_str()).unwrap_or_default().into()),
        ("doc", f.doc.as_ref().map_or(Value::Null, doc_value)),
        ("doc_realm", f.doc_realm.into()),
        (
            "realm_docs",
            array(&f.realm_docs, |(realm, doc)| object([("realm", (*realm).into()), ("summary", doc.summary().into()), ("description", doc.description.as_str().into()), ("doc", doc_value(doc))])),
        ),
        (
            "otherwise",
            f.otherwise.as_ref().map_or(Value::Null, |o| {
                object([
                    ("condition", o.condition.as_str().into()),
                    ("file", o.file.as_str().into()),
                    ("line", o.line.into()),
                    ("summary", o.doc.as_ref().map(DocBlock::summary).unwrap_or_default().into()),
                    ("description", o.doc.as_ref().map(|d| d.description.as_str()).unwrap_or_default().into()),
                    ("doc", o.doc.as_ref().map_or(Value::Null, doc_value)),
                ])
            }),
        ),
        ("callers", f.callers.iter().collect::<Vec<_>>().into()),
        ("hook_id", f.hook_id.as_ref().into()),
        ("implements", f.implements.as_ref().into()),
        ("category", f.category.as_str().into()),
        ("part", f.part.as_ref().into()),
        ("ty", f.ty.as_ref().into()),
        ("init", f.init.as_ref().into()),
        ("anchor", f.anchor.as_str().into()),
    ])
}

fn param_value(p: &Param) -> Value {
    object([("name", p.name.as_str().into()), ("type", p.ty.as_ref().into()), ("default", p.default.as_ref().into()), ("description", p.description.as_str().into())])
}

fn return_value(r: &Return) -> Value {
    object([("type", r.ty.as_ref().into()), ("description", r.description.as_str().into())])
}

/// The tags of a doc comment. Its summary and description sit next to it,
/// in the object it documents.
fn doc_value(d: &DocBlock) -> Value {
    let deprecated = d.is_deprecated().then(|| d.deprecation.clone().unwrap_or_default());
    object([
        ("params", array(&d.params, param_value)),
        ("returns", array(&d.returns, return_value)),
        (
            "variants",
            array(&d.variants, |v| {
                object([
                    ("signature", v.signature.as_str().into()),
                    ("description", v.description.as_str().into()),
                    ("params", array(&v.params, param_value)),
                    ("returns", array(&v.returns, return_value)),
                ])
            }),
        ),
        ("see", d.see.iter().collect::<Vec<_>>().into()),
        ("alias", d.aliases.iter().collect::<Vec<_>>().into()),
        ("deprecated", deprecated.into()),
        ("deprecation_version", d.deprecation_version.as_ref().into()),
        ("warning", d.warnings.first().map(|w| w.text.as_str()).into()),
        ("warnings", array(&d.warnings, |w| object([("label", w.label.as_ref().into()), ("text", w.text.as_str().into())]))),
        ("internal", d.is_internal().into()),
        ("environment", d.environment.as_ref().map(|e| e.name.as_str()).into()),
        ("environment_text", d.environment.as_ref().map(|e| e.text.as_str()).into()),
        ("realm", d.realm.into()),
        ("category", d.category.as_ref().into()),
        ("module", d.module.as_ref().into()),
        ("ignored", d.ignore.into()),
        ("examples", array(examples(&d.description), |(language, code)| object([("language", language.into()), ("code", code.into())]))),
        ("tags", array(&d.other, |(name, text)| object([("name", name.into()), ("text", text.into())]))),
    ])
}

/// The fenced code blocks of a Markdown text: the language after the
/// opening fence, if any, and the code with its common indentation
/// removed. A fence that is not closed runs to the end of the text.
fn examples(text: &str) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else { continue };
        let language = Some(info.trim().to_string()).filter(|l| !l.is_empty());
        let code: Vec<&str> = lines.by_ref().take_while(|l| !l.trim_start().starts_with("```")).collect();
        let indent = code.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        let code: Vec<&str> = code.iter().map(|l| l.get(indent..).unwrap_or("")).collect();
        out.push((language, code.join("\n")));
    }
    out
}

/// How deep arrays and objects may nest in [`parse`]'s input.
const MAX_DEPTH: usize = 256;

/// Parses a JSON document (RFC 8259). Numbers must be integers that fit an
/// `i64`, since nothing fdoc reads is fractional; anything else is an
/// error, as is nesting deeper than 256 levels.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at byte {}", self.i)
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected character"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth >= MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Str),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return Err(self.err("expected `,` or `]`")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.err("expected a member name"));
                    }
                    let key = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(self.err("expected `:`"));
                    }
                    self.i += 1;
                    self.ws();
                    members.push((key, self.value(depth + 1)?));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Object(members));
                        }
                        _ => return Err(self.err("expected `,` or `}`")),
                    }
                }
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        let digits = self.i;
        while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        if self.i == digits || (self.s[digits] == b'0' && self.i - digits > 1) {
            return Err(self.err("invalid number"));
        }
        if let Some(b'.' | b'e' | b'E') = self.s.get(self.i) {
            return Err(self.err("only integers are supported"));
        }
        // The bytes are ASCII digits with an optional sign.
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap_or_default();
        text.parse::<i64>().map(Value::Int).map_err(|_| self.err("integer out of range"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let hex = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        // `from_str_radix` would also take a sign.
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.err("invalid \\u escape"));
        }
        let hex = std::str::from_utf8(hex).map_err(|_| self.err("invalid \\u escape"))?;
        let n = u32::from_str_radix(hex, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.i += 4;
        Ok(n)
    }

    /// A string; `self.i` is at its opening quote.
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            // Copy the run up to the next quote, backslash or control
            // character in one go.
            let run = self.s[self.i..].iter().position(|&b| b == b'"' || b == b'\\' || b < 0x20).map_or(self.s.len(), |n| self.i + n);
            out.extend_from_slice(&self.s[self.i..run]);
            self.i = run;
            match self.s.get(self.i) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    // The input is a `&str` and escapes produce UTF-8.
                    return String::from_utf8(out).map_err(|_| self.err("invalid UTF-8"));
                }
                Some(b'\\') => {
                    self.i += 1;
                    let Some(&e) = self.s.get(self.i) else { return Err(self.err("unterminated string")) };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.s.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(self.err("unpaired surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("unpaired surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(code).ok_or_else(|| self.err("unpaired surrogate"))?
                        }
                        _ => return Err(self.err("invalid escape")),
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                Some(_) => return Err(self.err("control character in string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;
    use crate::model::{build, placed, BuildOptions};
    use crate::scanner::scan;
    use std::collections::HashMap;

    impl Value {
        /// The member `key` of an object, else `Null`.
        fn get(&self, key: &str) -> &Value {
            match self {
                Value::Object(members) => members.iter().find(|(k, _)| k == key).map_or(&Value::Null, |(_, v)| v),
                _ => &Value::Null,
            }
        }

        fn str(&self) -> &str {
            match self {
                Value::Str(s) => s,
                other => panic!("not a string: {other:?}"),
            }
        }

        fn items(&self) -> &[Value] {
            match self {
                Value::Array(items) => items,
                other => panic!("not an array: {other:?}"),
            }
        }

        /// The strings of an array of strings.
        fn strs(&self) -> Vec<&str> {
            self.items().iter().map(Value::str).collect()
        }
    }

    #[test]
    fn escapes_strings_as_ascii() {
        let mut out = String::new();
        escape("a\"b\\c\n\r\t\u{8}\u{c}\u{1}\u{7f}é€😀</", &mut out);
        assert_eq!(out, r#""a\"b\\c\n\r\t\b\f\u0001\u007fé€😀</""#.replace("é€😀", "\\u00e9\\u20ac\\ud83d\\ude00"));
        assert!(out.is_ascii());
        assert_eq!(parse(&out), Ok(Value::Str("a\"b\\c\n\r\t\u{8}\u{c}\u{1}\u{7f}é€😀</".to_string())));
    }

    #[test]
    fn writes_nested_values() {
        let v = object([("a", vec!["x", "y"].into()), ("b", Value::Array(vec![object([("c", Value::Null)])])), ("d", Value::Object(Vec::new())), ("e", Value::Array(Vec::new()))]);
        let mut out = String::new();
        write_value(&v, 0, &mut out);
        assert_eq!(out, "{\n  \"a\": [\"x\", \"y\"],\n  \"b\": [\n    {\n      \"c\": null\n    }\n  ],\n  \"d\": {},\n  \"e\": []\n}");
        assert_eq!(parse(&out), Ok(v));
    }

    #[test]
    fn extracts_examples() {
        let found = examples("Text.\n\n ```lua\n local x = 1\n   y()\n ```\nMore.\n```\nz()\n");
        assert_eq!(found, [(Some("lua".to_string()), "local x = 1\n  y()".to_string()), (None, "z()".to_string())]);
    }

    #[test]
    fn renders_the_project_as_json() {
        let files = vec![
            (
                "lib/a.lua".to_string(),
                scan("--- Class A, naïve.\nclass 'A'\n--- Does x.\n--\n-- ```\n-- A:do_x(1)\n-- ```\n-- @param n=1 [Number count]\n-- @return [Boolean ok, String why]\n-- @variant A:do_x(n, m)\n--   @param m [String mode]\n-- @see [A#other]\n-- @deprecation_version [0.9]\n-- @warning [Internal] Do not call.\nfunction A:do_x(n)\nend\nfunction A:other() end"),
            ),
            ("lib/sv_a.lua".to_string(), scan("function A:on_server()\n  --- Runs when spawned.\n  -- @param obj [Entity spawned]\n  hook.Run('Spawned', self, 1)\nend")),
            ("lib/sv_k.lua".to_string(), scan("--- Server twin.\nfunction A:twin() end")),
            ("lib/cl_k.lua".to_string(), scan("--- Client twin.\nfunction A:twin() end\nhook.Run('Spawned', LocalPlayer())")),
            ("lib/sh_gm.lua".to_string(), scan("--- Handles spawns.\nfunction GM:Spawned(obj, n) end")),
            ("lib/cl_add.lua".to_string(), scan("hook.Add('Spawned', 'my_id', |obj| -> obj)")),
            ("lib/sh_dev.lua".to_string(), scan("if is_development then\n  --- Logs.\n  -- @param msg [String]\n  function log(msg) end\nelse\n  --- Does nothing.\n  function log(msg) end\nend")),
        ];
        let layout = Layout::default_for("Test");
        let meta = HashMap::new();
        let project = build(placed(&layout, files), BuildOptions { title: Some("Test"), version: Some("1.0"), ..BuildOptions::new(&layout, &meta) });
        let text = render(&project);
        assert!(text.is_ascii() && text.ends_with("}\n"));
        let doc = parse(&text).unwrap();

        assert_eq!((doc.get("title").str(), doc.get("version").str()), ("Test", "1.0"));
        assert_eq!(doc.get("format_version"), &Value::Int(FORMAT_VERSION));
        let group = &doc.get("sections").items()[0].get("groups").items()[0];
        assert_eq!((group.get("path").str(), group.get("core")), ("core", &Value::Bool(true)));
        let modules = group.get("modules").items();
        let module = |id: &str| modules.iter().find(|m| m.get("id").str() == id).unwrap_or_else(|| panic!("no module {id}"));
        let function = |m: &Value, name: &str| m.get("functions").items().iter().find(|f| f.get("name").str() == name).unwrap_or_else(|| panic!("no function {name}")).clone();

        let a = module("A");
        assert_eq!((a.get("kind").str(), a.get("module_kind").str(), a.get("summary").str()), ("class", "class", "Class A, naïve."));
        let do_x = function(a, "do_x");
        assert_eq!((do_x.get("qualified").str(), do_x.get("signature").str(), do_x.get("kind").str()), ("A:do_x", "A:do_x(n)", "declared"));
        assert_eq!(do_x.get("params").strs(), ["n"]);
        assert_eq!((do_x.get("file").str(), do_x.get("line"), do_x.get("realm").str()), ("lib/a.lua", &Value::Int(15), "shared"));
        let d = do_x.get("doc");
        let param = &d.get("params").items()[0];
        assert_eq!((param.get("name").str(), param.get("type").str(), param.get("default").str(), param.get("description").str()), ("n", "Number", "1", "count"));
        let returns: Vec<(&str, &str)> = d.get("returns").items().iter().map(|r| (r.get("type").str(), r.get("description").str())).collect();
        assert_eq!(returns, [("Boolean", "ok"), ("String", "why")]);
        assert_eq!(d.get("variants").items()[0].get("signature").str(), "A:do_x(n, m)");
        assert_eq!(d.get("see").strs(), ["A#other"]);
        assert_eq!((d.get("deprecated").str(), d.get("deprecation_version").str()), ("", "0.9"));
        assert_eq!((d.get("warning").str(), d.get("internal")), ("Do not call.", &Value::Bool(true)));
        assert_eq!(d.get("examples").items()[0].get("code").str(), "A:do_x(1)");
        assert_eq!(function(a, "other").get("doc"), &Value::Null);

        let twin = function(a, "twin");
        assert_eq!(twin.get("realm").str(), "shared");
        let realms: Vec<&str> = twin.get("sources").items().iter().map(|s| s.get("realm").str()).collect();
        assert_eq!(realms, ["server", "client"]);
        let realm_docs: Vec<(&str, &str)> = twin.get("realm_docs").items().iter().map(|d| (d.get("realm").str(), d.get("summary").str())).collect();
        assert_eq!((twin.get("summary").str(), realm_docs), ("Server twin.", vec![("client", "Client twin.")]));

        let hooks = module("Hooks");
        assert_eq!(hooks.get("kind").str(), "hooks");
        let spawned = function(hooks, "Spawned");
        assert_eq!((spawned.get("kind").str(), spawned.get("summary").str()), ("hook", "Runs when spawned."));
        assert_eq!(spawned.get("params").strs(), ["self", "1"]);
        assert_eq!(spawned.get("doc").get("params").items()[0].get("type").str(), "Entity");
        assert_eq!(spawned.get("callers").strs(), ["A:on_server"]);
        let sites: Vec<(&str, &Value)> = spawned.get("sources").items().iter().map(|s| (s.get("file").str(), s.get("line"))).collect();
        assert_eq!(sites, [("lib/sv_a.lua", &Value::Int(4)), ("lib/cl_k.lua", &Value::Int(3))]);

        let gm = module("GM");
        assert_eq!((gm.get("kind").str(), gm.get("object").str()), ("gamemode", "GM"));
        assert_eq!(function(gm, "Spawned").get("implements").str(), "hook:Spawned");
        let added = function(module("hook.Add"), "Spawned");
        assert_eq!((added.get("kind").str(), added.get("hook_id").str()), ("hook_add", "my_id"));
        assert_eq!(added.get("params").strs(), ["obj"]);

        let log = function(module("Globals"), "log");
        let otherwise = log.get("otherwise");
        assert_eq!((otherwise.get("condition").str(), otherwise.get("summary").str()), ("is_development", "Does nothing."));
        assert_eq!(doc.get("index").get("A#do_x").str(), "core/A.html#do_x");
    }

    #[test]
    fn parses_json() {
        let v = parse(r#" {"a": [1, -2, true, false, null], "b": "x\u00e9\ud83d\ude00\n\"", "c": {}} "#).unwrap();
        assert_eq!(
            v,
            Value::Object(vec![
                ("a".into(), Value::Array(vec![Value::Int(1), Value::Int(-2), Value::Bool(true), Value::Bool(false), Value::Null])),
                ("b".into(), Value::Str("x\u{e9}\u{1F600}\n\"".into())),
                ("c".into(), Value::Object(vec![])),
            ])
        );
        for bad in ["", "[1,]", "{\"a\" 1}", "\"abc", "01", "1.5", "[1] x", "\"\\ud800\"", "\"a\tb\"", "tru"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&"[".repeat(300)).is_err());
        // 256 levels nest, 257 do not.
        assert!(parse(&format!("{}{}", "[".repeat(256), "]".repeat(256))).is_ok());
        assert!(parse(&format!("{}{}", "[".repeat(257), "]".repeat(257))).is_err());
        for bad in ["\"\\u+04a\"", "\"\\u-04a\"", "\"\\u 04a\""] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let mut s = String::new();
        escape("d\u{e9}j\u{e0} \u{1F600} \"q\"", &mut s);
        assert_eq!(parse(&s).unwrap(), Value::Str("d\u{e9}j\u{e0} \u{1F600} \"q\"".into()));
    }
}
