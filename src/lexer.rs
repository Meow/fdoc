//! A small tokenizer for Lua source (including the GLua extensions: `!`,
//! `!=`, `&&`, `||`, `//` and `/* */` comments, `continue`).
//!
//! The tokenizer only needs to be precise enough to find declarations and
//! the comments attached to them, so it does not validate the source.

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

pub fn tokenize(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut line_start = true;

    while i < b.len() {
        let c = b[i];

        // Whitespace.
        if c == b'\n' {
            line += 1;
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let start_line = line;
        let at_line_start = line_start;
        line_start = false;

        // Comments: `--`, `--[[ ]]`, `//`, `/* */`.
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            i += 2;
            if let Some(level) = long_bracket_level(b, i) {
                let open_len = level + 2;
                let (text, end, nl) = read_long(b, i + open_len, level);
                line += nl;
                out.push(Token { tok: Tok::Comment { text, long: true }, line: start_line, line_start: at_line_start });
                i = end;
            } else {
                let end = line_end(b, i);
                let text = String::from_utf8_lossy(&b[i..end]).into_owned();
                out.push(Token { tok: Tok::Comment { text, long: false }, line: start_line, line_start: at_line_start });
                i = end;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = line_end(b, i + 2);
            let text = String::from_utf8_lossy(&b[i + 2..end]).into_owned();
            out.push(Token { tok: Tok::Comment { text, long: false }, line: start_line, line_start: at_line_start });
            i = end;
            continue;
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
            let text = String::from_utf8_lossy(&b[i + 2..j.min(b.len())]).into_owned();
            out.push(Token { tok: Tok::Comment { text, long: true }, line: start_line, line_start: at_line_start });
            line += nl;
            i = (j + 2).min(b.len());
            continue;
        }

        // Strings.
        if c == b'\'' || c == b'"' {
            let mut j = i + 1;
            let mut s = Vec::new();
            while j < b.len() && b[j] != c {
                if b[j] == b'\\' && j + 1 < b.len() {
                    match b[j + 1] {
                        b'n' => s.push(b'\n'),
                        b't' => s.push(b'\t'),
                        b'\n' => {
                            s.push(b'\n');
                            line += 1;
                        }
                        other => s.push(other),
                    }
                    j += 2;
                    continue;
                }
                if b[j] == b'\n' {
                    // Unterminated string; stop at the end of the line.
                    break;
                }
                s.push(b[j]);
                j += 1;
            }
            out.push(Token { tok: Tok::Str(String::from_utf8_lossy(&s).into_owned()), line: start_line, line_start: at_line_start });
            i = (j + 1).min(b.len());
            continue;
        }
        if let (b'[', Some(level)) = (c, long_bracket_level(b, i)) {
            let open_len = level + 2;
            let (text, end, nl) = read_long(b, i + open_len, level);
            line += nl;
            out.push(Token { tok: Tok::Str(text), line: start_line, line_start: at_line_start });
            i = end;
            continue;
        }

        // Numbers.
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || ((b[j] == b'-' || b[j] == b'+') && matches!(b[j - 1], b'e' | b'E' | b'p' | b'P'))) {
                j += 1;
            }
            out.push(Token { tok: Tok::Number(src[i..j].to_string()), line: start_line, line_start: at_line_start });
            i = j;
            continue;
        }

        // Names and keywords.
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            out.push(Token { tok: Tok::Name(String::from_utf8_lossy(&b[i..j]).into_owned()), line: start_line, line_start: at_line_start });
            i = j;
            continue;
        }

        // Symbols, longest match first.
        const SYMS: [&str; 15] = ["...", "..", "==", "~=", "!=", "<=", ">=", "&&", "||", "::", "<<", ">>", "//", "=>", "->"];
        let rest = &src[i..];
        let mut matched = None;
        for s in SYMS {
            if rest.starts_with(s) {
                matched = Some(s);
                break;
            }
        }
        let sym = match matched {
            Some(s) => s.to_string(),
            None => {
                let ch = rest.chars().next().unwrap();
                ch.to_string()
            }
        };
        i += sym.len();
        out.push(Token { tok: Tok::Sym(sym), line: start_line, line_start: at_line_start });
    }

    out
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
    fn tokenizes_glua_extensions() {
        let toks = tokenize("if !a && b != c then continue end // c-style\n/* block\n*/ d");
        assert!(toks[1].is_sym("!"));
        assert!(toks[3].is_sym("&&"));
        assert!(toks[5].is_sym("!="));
        assert!(toks.iter().any(|t| matches!(&t.tok, Tok::Comment { text, long: false } if text == " c-style")));
        assert!(toks.iter().any(|t| matches!(&t.tok, Tok::Comment { text, long: true } if text == " block\n")));
        assert_eq!(toks.last().unwrap().line, 3);
    }
}
