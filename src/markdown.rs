//! Markdown rendering for doc comments, built on comrak.
//!
//! Two things are layered on top of CommonMark: inline code that names a
//! documented function or module becomes a link, and fenced code blocks are
//! syntax-coloured as Lua.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use comrak::adapters::SyntaxHighlighterAdapter;
use comrak::nodes::{Ast, LineColumn, NodeLink, NodeValue};
use comrak::options::Plugins;
use comrak::{format_html_with_plugins, parse_document, Arena, Options};

use crate::html::esc;

pub type Resolver<'a> = &'a dyn Fn(&str) -> Option<String>;

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.autolink = true;
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.render.escape = true;
    options
}

struct LuaHighlighter;

impl SyntaxHighlighterAdapter for LuaHighlighter {
    fn write_highlighted(&self, output: &mut dyn fmt::Write, lang: Option<&str>, code: &str) -> fmt::Result {
        match lang {
            None | Some("") | Some("lua") | Some("glua") => output.write_str(&highlight_lua(code)),
            _ => output.write_str(&esc(code)),
        }
    }

    fn write_pre_tag(&self, output: &mut dyn fmt::Write, _attributes: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        output.write_str("<pre>")
    }

    fn write_code_tag(&self, output: &mut dyn fmt::Write, attributes: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        let class = attributes.get("class").map(|c| c.to_string()).unwrap_or_else(|| "language-lua".to_string());
        write!(output, "<code class=\"{}\">", esc(&class))
    }
}

/// Renders a Markdown document to HTML.
pub fn render(text: &str, resolve: Resolver) -> String {
    let arena = Arena::new();
    let options = options();
    let root = parse_document(&arena, text, &options);

    // Link code spans that name something documented, and demote headings
    // so they sit below the page's own headings.
    let nodes: Vec<_> = root.descendants().collect();
    for node in nodes {
        let replacement = {
            let data = node.data();
            match &data.value {
                NodeValue::Code(code) => resolve(code.literal.trim()),
                _ => None,
            }
        };
        if let Some(url) = replacement {
            let link = arena.alloc(Ast::new(NodeValue::Link(Box::new(NodeLink { url, title: String::new() })), LineColumn { line: 1, column: 1 }).into());
            node.insert_before(link);
            node.detach();
            link.append(node);
            continue;
        }
        let mut data = node.data_mut();
        if let NodeValue::Heading(h) = &mut data.value {
            h.level = (h.level + 3).min(6);
        }
    }

    let highlighter = LuaHighlighter;
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&highlighter);

    let mut out = String::new();
    format_html_with_plugins(root, &options, &mut out, &plugins).expect("writing to a String cannot fail");
    out
}

/// Renders a single line of Markdown without the wrapping paragraph.
pub fn inline(text: &str, resolve: Resolver) -> String {
    let html = render(text, resolve);
    let trimmed = html.trim();
    match trimmed.strip_prefix("<p>").and_then(|s| s.strip_suffix("</p>")) {
        Some(inner) if !inner.contains("<p>") => inner.to_string(),
        _ => trimmed.to_string(),
    }
}

const LUA_KEYWORDS: [&str; 24] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while", "continue", "self",
];

/// Wraps Lua tokens in spans for syntax colouring.
pub fn highlight_lua(code: &str) -> String {
    let b = code.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let push = |out: &mut String, class: &str, s: &str| {
        out.push_str(&format!("<span class=\"hl-{class}\">{}</span>", esc(s)));
    };
    while i < b.len() {
        let c = b[i];
        if c == b'-' && b.get(i + 1) == Some(&b'-') || c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = code[i..].find('\n').map(|p| p + i).unwrap_or(b.len());
            push(&mut out, "c", &code[i..end]);
            i = end;
            continue;
        }
        if c == b'\'' || c == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c && b[j] != b'\n' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(b.len());
            push(&mut out, "s", &code[i..end]);
            i = end;
            continue;
        }
        if c == b'[' && (b.get(i + 1) == Some(&b'[') || b.get(i + 1) == Some(&b'=')) {
            let mut j = i + 1;
            while b.get(j) == Some(&b'=') {
                j += 1;
            }
            if b.get(j) == Some(&b'[') {
                let close = format!("]{}]", "=".repeat(j - i - 1));
                let end = code[j + 1..].find(&close).map(|p| p + j + 1 + close.len()).unwrap_or(b.len());
                push(&mut out, "s", &code[i..end]);
                i = end;
                continue;
            }
        }
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.') {
                j += 1;
            }
            push(&mut out, "n", &code[i..j]);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let word = &code[i..j];
            if LUA_KEYWORDS.contains(&word) {
                push(&mut out, "k", word);
            } else if b.get(j) == Some(&b'(') {
                push(&mut out, "f", word);
            } else {
                out.push_str(&esc(word));
            }
            i = j;
            continue;
        }
        let ch = code[i..].chars().next().unwrap();
        out.push_str(&esc(&ch.to_string()));
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn renders_blocks() {
        let html = render("Summary line.\nMore text.\n\n ```\n local x = 1 -- hi\n ```\n\n* one\n* two\n\n# Heading\n\n<b>raw</b>", &none);
        assert!(html.starts_with("<p>Summary line.\nMore text.</p>"), "{html}");
        assert!(html.contains("<pre><code class=\"language-lua\"><span class=\"hl-k\">local</span> x = <span class=\"hl-n\">1</span> <span class=\"hl-c\">-- hi</span>\n</code></pre>"), "{html}");
        assert!(html.contains("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"), "{html}");
        assert!(html.contains("<h4>Heading</h4>"), "{html}");
        assert!(html.contains("&lt;b&gt;raw&lt;/b&gt;"), "{html}");
    }

    #[test]
    fn renders_inline() {
        let resolve = |s: &str| if s == "Foo.bar" { Some("x.html#bar".to_string()) } else { None };
        let html = inline("Call `Foo.bar` or `baz` with **bold**, see [docs](http://a.b) or https://x.y/z.", &resolve);
        assert_eq!(
            html,
            "Call <a href=\"x.html#bar\"><code>Foo.bar</code></a> or <code>baz</code> with <strong>bold</strong>, see <a href=\"http://a.b\">docs</a> or <a href=\"https://x.y/z\">https://x.y/z</a>."
        );
        assert_eq!(inline("a < b & snake_case_name", &none), "a &lt; b &amp; snake_case_name");
    }
}
