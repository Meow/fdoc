//! A small tokenizer for Lua source (including the GLua extensions: `!`,
//! `!=`, `&&`, `||`, `//` and `/* */` comments, `continue`).
//!
//! The tokenizer only needs to be precise enough to find declarations and
//! the comments attached to them, so it does not validate the source.
//! Lumen templates (`.lumen`) are tokenized with their markup skipped and
//! the Lua code inside it kept.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Identifier or keyword.
    Name(String),
    /// String literal, with the delimiters removed.
    Str(String),
    /// Numeric literal, as written.
    Number(String),
    /// Comment text, with the comment delimiters removed.
    Comment { text: String, long: bool },
    /// Punctuation / operator.
    Sym(String),
    /// An element of a Lumen template, by its tag name (empty for a
    /// fragment). The tokens of the Lua code in its splices follow it.
    Markup(String),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    /// 1-based line on which the token starts.
    pub line: usize,
    /// True when only whitespace precedes the token on its line.
    pub line_start: bool,
}

impl Token {
    pub fn is_name(&self, s: &str) -> bool {
        matches!(&self.tok, Tok::Name(n) if n == s)
    }

    pub fn is_sym(&self, s: &str) -> bool {
        matches!(&self.tok, Tok::Sym(n) if n == s)
    }

    pub fn name(&self) -> Option<&str> {
        match &self.tok {
            Tok::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn string(&self) -> Option<&str> {
        match &self.tok {
            Tok::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The language a source file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Lua,
    /// A Lumen template: Lua in which markup such as
    /// `<view style={s}>Hi, {name}</view>` may stand wherever an expression
    /// may.
    Lumen,
    /// NL (`.nl`, `.nlua`), which compiles to Lua. It is read with NL's own
    /// parser (see `crate::nl`), not with this lexer.
    Nl,
}

impl Syntax {
    /// `Lumen` for a `.lumen` file, `Nl` for a `.nl` or `.nlua` file, `Lua`
    /// for anything else.
    pub fn of_path(path: &str) -> Syntax {
        if path.ends_with(".lumen") {
            Syntax::Lumen
        } else if path.ends_with(".nl") || path.ends_with(".nlua") {
            Syntax::Nl
        } else {
            Syntax::Lua
        }
    }
}

/// The name of the Lua file a file stands for: an NL file `x.nl` or
/// `x.nlua` is `x.lua` wherever Garry's Mod looks for Lua (with NL's
/// `lua_shared`). Other names are returned as they are.
pub fn lua_name(name: &str) -> std::borrow::Cow<'_, str> {
    match name.strip_suffix(".nlua").or_else(|| name.strip_suffix(".nl")) {
        Some(stem) => format!("{stem}.lua").into(),
        None => name.into(),
    }
}

/// Tokenizes source written in `syntax`. In a Lumen template each element
/// is one `Tok::Markup` token, followed by the tokens of the Lua code in its
/// `{ }` splices (attribute values, `{...spread}` and child expressions) in
/// source order. Tag and attribute names, quoted attribute values, text and
/// `<!-- -->` comments produce no tokens.
pub fn tokenize_as(src: &str, syntax: Syntax) -> Vec<Token> {
    let mut lexer = Lexer { src, b: src.as_bytes(), i: 0, line: 1, line_start: true, lumen: syntax == Syntax::Lumen, out: Vec::new() };
    lexer.lua(false);
    lexer.out
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    /// The current position in `b`.
    i: usize,
    line: usize,
    /// Only whitespace precedes the current position on its line.
    line_start: bool,
    /// The source is a Lumen template, so `<` can start markup.
    lumen: bool,
    out: Vec<Token>,
}

impl Lexer<'_> {
    /// Lexes Lua code up to the end of the source or, for a splice, up to
    /// the `}` that closes it, which is skipped. In a Lumen template a `<`
    /// starts markup when a name or `>` follows it and the token before it
    /// cannot end an operand, as in the Lumen compiler: `a < b` stays a
    /// comparison, `return <view/>` is markup.
    fn lua(&mut self, splice: bool) {
        let mut operand = false;
        let mut depth = 0usize;
        loop {
            self.skip_space();
            let Some(c) = self.peek(0) else { return };
            if self.lumen && !operand && self.at_element() {
                self.element();
                operand = true;
                continue;
            }
            if splice && c == b'}' && depth == 0 {
                self.skip(1);
                return;
            }

            let line = self.line;
            let line_start = self.line_start;
            self.line_start = false;
            let tok = self.token();
            match &tok {
                Tok::Comment { .. } => {}
                Tok::Sym(s) if s == "{" => {
                    depth += 1;
                    operand = false;
                }
                Tok::Sym(s) if s == "}" => {
                    depth = depth.saturating_sub(1);
                    operand = true;
                }
                tok => operand = ends_operand(tok),
            }
            self.out.push(Token { tok, line, line_start });
        }
    }

    /// Reads the token that starts at the current position, which is not
    /// whitespace, and moves past it.
    fn token(&mut self) -> Tok {
        let b = self.b;
        let i = self.i;
        let c = b[i];

        // Comments: `--`, `--[[ ]]`, `//`, `/* */`.
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            let start = i + 2;
            if let Some(level) = long_bracket_level(b, start) {
                let open_len = level + 2;
                let (text, end, nl) = read_long(b, start + open_len, level);
                self.line += nl;
                self.i = end;
                return Tok::Comment { text, long: true };
            }
            let end = line_end(b, start);
            self.i = end;
            return Tok::Comment { text: String::from_utf8_lossy(&b[start..end]).into_owned(), long: false };
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = line_end(b, i + 2);
            self.i = end;
            return Tok::Comment { text: String::from_utf8_lossy(&b[i + 2..end]).into_owned(), long: false };
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            let mut nl = 0;
            while j < b.len() && !(b[j] == b'*' && b.get(j + 1) == Some(&b'/')) {
                if b[j] == b'\n' {
                    nl += 1;
                }
                j += 1;
            }
            let text = String::from_utf8_lossy(&b[i + 2..j]).into_owned();
            self.line += nl;
            self.i = (j + 2).min(b.len());
            return Tok::Comment { text, long: true };
        }

        // Strings. An unterminated one ends at the end of its line, which
        // is left for the next token so the line count stays right.
        if c == b'\'' || c == b'"' {
            let mut j = i + 1;
            let mut s = Vec::new();
            while j < b.len() && b[j] != c && b[j] != b'\n' {
                if b[j] == b'\\' && j + 1 < b.len() {
                    match b[j + 1] {
                        b'n' => s.push(b'\n'),
                        b't' => s.push(b'\t'),
                        b'\n' => {
                            s.push(b'\n');
                            self.line += 1;
                        }
                        other => s.push(other),
                    }
                    j += 2;
                    continue;
                }
                s.push(b[j]);
                j += 1;
            }
            self.i = if b.get(j) == Some(&c) { j + 1 } else { j };
            return Tok::Str(String::from_utf8_lossy(&s).into_owned());
        }
        if let (b'[', Some(level)) = (c, long_bracket_level(b, i)) {
            let open_len = level + 2;
            let (text, end, nl) = read_long(b, i + open_len, level);
            self.line += nl;
            self.i = end;
            return Tok::Str(text);
        }

        // Numbers.
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || ((b[j] == b'-' || b[j] == b'+') && matches!(b[j - 1], b'e' | b'E' | b'p' | b'P'))) {
                j += 1;
            }
            self.i = j;
            return Tok::Number(self.src[i..j].to_string());
        }

        // Names and keywords.
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            self.i = j;
            return Tok::Name(String::from_utf8_lossy(&b[i..j]).into_owned());
        }

        // Symbols, longest match first.
        const SYMS: [&str; 15] = ["...", "..", "==", "~=", "!=", "<=", ">=", "&&", "||", "::", "<<", ">>", "//", "=>", "->"];
        let rest = &self.src[i..];
        let sym = match SYMS.into_iter().find(|s| rest.starts_with(s)) {
            Some(s) => s.to_string(),
            None => rest.chars().next().unwrap().to_string(),
        };
        self.i += sym.len();
        Tok::Sym(sym)
    }

    /// True when a Lumen element starts at the current position: a `<`
    /// followed by a name or `>`.
    fn at_element(&self) -> bool {
        self.peek(0) == Some(b'<') && self.peek(1).is_some_and(|n| n == b'>' || is_name_start(n))
    }

    /// Skips the Lumen element whose `<` is at the current position, with
    /// its attributes and children, after emitting its `Tok::Markup` token.
    /// The Lua code of its splices is lexed as it comes. Malformed markup is
    /// skipped as far as it goes.
    fn element(&mut self) {
        let (line, line_start) = (self.line, self.line_start);
        self.skip(1);
        let fragment = self.peek(0) == Some(b'>');
        let tag = if fragment {
            self.skip(1);
            String::new()
        } else {
            self.markup_name()
        };
        self.out.push(Token { tok: Tok::Markup(tag), line, line_start });
        if fragment || self.attributes() {
            self.children();
        }
    }

    /// Skips the attributes of a tag through the `>` that ends it. False
    /// when no children follow: the tag closes itself (`/>`) or the source
    /// ends.
    fn attributes(&mut self) -> bool {
        loop {
            self.skip_space();
            match self.peek(0) {
                None => return false,
                Some(b'/') if self.peek(1) == Some(b'>') => {
                    self.skip(2);
                    return false;
                }
                Some(b'>') => {
                    self.skip(1);
                    return true;
                }
                Some(b'{') => {
                    let spread = self.b[self.i..].starts_with(b"{...");
                    self.splice(if spread { 4 } else { 1 });
                }
                Some(c) if is_name_start(c) => {
                    self.markup_name();
                    self.skip_space();
                    if self.peek(0) != Some(b'=') {
                        continue;
                    }
                    self.skip(1);
                    self.skip_space();
                    match self.peek(0) {
                        Some(quote @ (b'"' | b'\'')) => {
                            let len = self.b[self.i + 1..].iter().position(|&ch| ch == quote).map_or(self.b.len() - self.i, |p| p + 2);
                            self.skip(len);
                        }
                        Some(b'{') => self.splice(1),
                        _ => {}
                    }
                }
                Some(_) => self.skip(1),
            }
        }
    }

    /// Skips the children of an element through its closing tag: text,
    /// `<!-- -->` comments, nested elements and splices.
    fn children(&mut self) {
        let b = self.b;
        loop {
            let rest = &b[self.i..];
            if rest.is_empty() {
                return;
            }
            if rest.starts_with(b"<!--") {
                let len = rest[4..].windows(3).position(|w| w == b"-->").map_or(rest.len(), |p| p + 7);
                self.skip(len);
            } else if rest.starts_with(b"</") {
                let len = rest.iter().position(|&ch| ch == b'>').map_or(rest.len(), |p| p + 1);
                self.skip(len);
                return;
            } else if self.at_element() {
                self.element();
            } else if rest[0] == b'{' {
                self.splice(1);
            } else {
                let len = rest[1..].iter().position(|&ch| ch == b'<' || ch == b'{').map_or(rest.len(), |p| p + 1);
                self.skip(len);
            }
        }
    }

    /// Lexes the Lua code of the splice at the current position, whose
    /// opening (`{`, or `{...` for a spread) is `open` bytes long, through
    /// its closing `}`.
    fn splice(&mut self, open: usize) {
        self.skip(open);
        self.lua(true);
    }

    /// Reads a tag or attribute name of markup (letters, digits, `_`, `.`
    /// and `-`).
    fn markup_name(&mut self) -> String {
        let start = self.i;
        let len = self.b[start..].iter().position(|&c| !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))).unwrap_or(self.b.len() - start);
        self.skip(len);
        self.src[start..start + len].to_string()
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.b.get(self.i + offset).copied()
    }

    /// Moves `n` bytes ahead, counting lines.
    fn skip(&mut self, n: usize) {
        let end = (self.i + n).min(self.b.len());
        for &c in &self.b[self.i..end] {
            if c == b'\n' {
                self.line += 1;
                self.line_start = true;
            } else if !c.is_ascii_whitespace() {
                self.line_start = false;
            }
        }
        self.i = end;
    }

    fn skip_space(&mut self) {
        let len = self.b[self.i..].iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(self.b.len() - self.i);
        self.skip(len);
    }
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// True for a Lua keyword, GLua's `continue` included.
pub fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "and" | "break" | "continue" | "do" | "else" | "elseif" | "end" | "false" | "for" | "function" | "goto" | "if" | "in" | "local" | "nil" | "not" | "or" | "repeat" | "return" | "then" | "true" | "until" | "while"
    )
}

/// True when a token can end an operand: names other than keywords, `end`,
/// `true`, `false` and `nil`, literals, markup, `)`, `]`, `}` and `...`.
/// After one, `(` is a call, `-` a subtraction and `<` a comparison rather
/// than markup, as in the Lumen compiler.
pub fn ends_operand(tok: &Tok) -> bool {
    match tok {
        Tok::Name(n) => !is_keyword(n) || matches!(n.as_str(), "end" | "nil" | "true" | "false"),
        Tok::Str(_) | Tok::Number(_) | Tok::Markup(_) => true,
        Tok::Sym(s) => matches!(s.as_str(), ")" | "]" | "}" | "..."),
        Tok::Comment { .. } => false,
    }
}

/// If `b[i..]` starts a long bracket (`[`, `[=`, `[==`...), returns its level.
fn long_bracket_level(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    let mut level = 0;
    while b.get(j) == Some(&b'=') {
        level += 1;
        j += 1;
    }
    if b.get(j) == Some(&b'[') { Some(level) } else { None }
}

/// Reads a long string/comment body starting right after the opening bracket.
/// Returns the text, the index after the closing bracket and the newline count.
fn read_long(b: &[u8], start: usize, level: usize) -> (String, usize, usize) {
    let mut j = start;
    let mut nl = 0;
    while j < b.len() {
        if b[j] == b']' {
            let mut k = j + 1;
            let mut eq = 0;
            while b.get(k) == Some(&b'=') {
                eq += 1;
                k += 1;
            }
            if eq == level && b.get(k) == Some(&b']') {
                let text = String::from_utf8_lossy(&b[start..j]).into_owned();
                return (text, k + 1, nl);
            }
        }
        if b[j] == b'\n' {
            nl += 1;
        }
        j += 1;
    }
    let text = String::from_utf8_lossy(&b[start..]).into_owned();
    (text, b.len(), nl)
}

fn line_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(src: &str) -> Vec<Token> {
        tokenize_as(src, Syntax::Lua)
    }

    #[test]
    fn tokenizes_comments_and_strings() {
        let toks = tokenize("--- doc\nfunction a.b:c(x, y) return 'q\\'s' end -- trailing\n--[[ long\ncomment ]] x = 1.5e-3");
        assert!(matches!(&toks[0].tok, Tok::Comment { text, long: false } if text == "- doc"));
        assert!(toks[0].line_start);
        assert!(toks[1].is_name("function") && toks[1].line == 2);
        let s = toks.iter().find_map(|t| t.string()).unwrap();
        assert_eq!(s, "q's");
        let trailing = toks.iter().find(|t| matches!(&t.tok, Tok::Comment { text, .. } if text == " trailing")).unwrap();
        assert!(!trailing.line_start);
        let long = toks.iter().find(|t| matches!(&t.tok, Tok::Comment { long: true, .. })).unwrap();
        assert_eq!(long.line, 3);
        let last = toks.last().unwrap();
        assert!(matches!(&last.tok, Tok::Number(n) if n == "1.5e-3"));
        assert_eq!(last.line, 4);
    }

    #[test]
    fn unterminated_strings_keep_the_line_count() {
        let toks = tokenize("x = 'open\ny = 1");
        let y = toks.iter().find(|t| t.is_name("y")).unwrap();
        assert_eq!((y.line, y.line_start), (2, true));
    }

    #[test]
    fn tokenizes_glua_extensions() {
        let toks = tokenize("if !a && b != c then continue end // c-style\n/* block\n*/ d");
        assert!(toks[1].is_sym("!"));
        assert!(toks[3].is_sym("&&"));
        assert!(toks[5].is_sym("!="));
        assert!(toks.iter().any(|t| matches!(&t.tok, Tok::Comment { text, long: false } if text == " c-style")));
        assert!(toks.iter().any(|t| matches!(&t.tok, Tok::Comment { text, long: true } if text == " block\n")));
        assert_eq!(toks.last().unwrap().line, 3);
    }

    fn lumen(src: &str) -> Vec<Token> {
        tokenize_as(src, Syntax::Lumen)
    }

    /// The tokens as compact text, comments left out: names, numbers and
    /// symbols as written, strings quoted, markup as `<tag>`.
    fn texts(toks: &[Token]) -> Vec<String> {
        toks.iter()
            .filter_map(|t| match &t.tok {
                Tok::Name(s) | Tok::Number(s) | Tok::Sym(s) => Some(s.clone()),
                Tok::Str(s) => Some(format!("'{s}'")),
                Tok::Markup(tag) => Some(format!("<{tag}>")),
                Tok::Comment { .. } => None,
            })
            .collect()
    }

    #[test]
    fn skips_lumen_markup_and_lexes_splices() {
        let toks = lumen(
            "return <view style={{ gap = 8 }} on_press={function() hide() end} wide label=\"it's {not} -- code\">
  Text with end, function, 'quotes', \"doubles\", -- dashes and } braces.
  <!-- a comment with end and <view> -->
  <text>{name}</text>
  <Flux.Card {...props} key='a' />
  <>{#items > 0 and 'some'}</>
</view>
function after() end",
        );
        assert_eq!(
            texts(&toks),
            [
                "return", "<view>", "{", "gap", "=", "8", "}", "function", "(", ")", "hide", "(", ")", "end", "<text>", "name", "<Flux.Card>", "props", "<>", "#", "items", ">", "0", "and", "'some'", "function",
                "after", "(", ")", "end"
            ]
        );
        assert!(!toks.iter().any(|t| matches!(t.tok, Tok::Comment { .. })), "`--` in markup is not a comment");
        let after = toks.iter().find(|t| t.is_name("after")).unwrap();
        assert_eq!(after.line, 8);
        assert!(toks.iter().rev().find(|t| t.is_name("function")).is_some_and(|t| t.line == 8 && t.line_start));
        assert!(toks.iter().find(|t| t.is_name("name")).is_some_and(|t| t.line == 4 && !t.line_start), "a splice after a tag does not start its line");
    }

    #[test]
    fn tells_markup_from_comparisons() {
        let markup = |src: &str| lumen(src).iter().filter(|t| matches!(t.tok, Tok::Markup(_))).count();
        assert_eq!(markup("if a < b and #list < 3 and f(x)<y and t[1] <z and 'a' < 'b' and 1 <c then end"), 0);
        assert_eq!(markup("x = a <= b or a << b or a < (b) or a < 1"), 0);
        assert_eq!(markup("return <view/>"), 1);
        assert_eq!(markup("f(<view/>, <text/>)"), 2);
        assert_eq!(markup("x = cond and <view/> or nil"), 1);
        assert_eq!(markup("local t = { <view/>, a = <>x</> }"), 2);
        assert_eq!(markup("if done then end <view/>"), 0, "`end` ends an operand");
        assert_eq!(markup("x = y -- <view>\n<view/>"), 0, "comments do not change the token before");
        assert_eq!(tokenize("return <view/>")[1].tok, Tok::Sym("<".to_string()), "Lua files have no markup");
    }

    #[test]
    fn lexes_markup_nested_in_splices() {
        let toks = lumen("local x = <view>{Lumen.map(list, function(item)\n  return <Row key={item.id} on_press={function() f('}') end} />\nend)}</view>\nlocal y = 1");
        assert_eq!(
            texts(&toks),
            [
                "local", "x", "=", "<view>", "Lumen", ".", "map", "(", "list", ",", "function", "(", "item", ")", "return", "<Row>", "item", ".", "id", "function", "(", ")", "f", "(", "'}'", ")", "end", "end", ")",
                "local", "y", "=", "1"
            ]
        );
        let lines: Vec<usize> = toks.iter().filter(|t| t.is_name("end") || t.is_name("local")).map(|t| t.line).collect();
        assert_eq!(lines, [1, 2, 3, 4]);
    }

    #[test]
    fn counts_lines_through_markup() {
        let toks = lumen("return <view\n  title='one\ntwo'\n  style={{\n    a = 1\n  }}>\n  text\n  <!--\n  -->\n  {b}\n</view>\nc");
        let line = |name: &str| toks.iter().find(|t| t.is_name(name)).map(|t| t.line);
        assert_eq!((line("a"), line("b"), line("c")), (Some(5), Some(10), Some(12)));
        assert!(toks.iter().find(|t| t.is_name("c")).unwrap().line_start);
        let unclosed = lumen("return <view a={x}>\ntext");
        assert_eq!(texts(&unclosed), ["return", "<view>", "x"], "unclosed markup runs to the end");
    }
}
