//! Parses the text of a documentation comment block into its parts.
//!
//! A block looks like this:
//!
//! ```text
//! --- Summary line, followed by a longer description in Markdown.
//! -- @param name=default [Type description]
//! -- @return [Type description, Type description]
//! -- @variant name(a)
//! --   @param a [Type]
//! --   @return [Type]
//! -- @see [Owner#name]
//! -- @warning [Internal] optional text
//! -- @deprecation [reason]
//! -- @deprecation_version [0.8.0]
//! -- @alias [Other.name]
//! -- @category [Name]
//! -- @realm [server|client|shared]
//! -- @module [Name]
//! -- @ignore
//! ```
//!
//! Indented `@param` and `@return` lines under a `@variant` belong to it.
//! The summary is the first sentence of the description.

/// Where a piece of code runs in Garry's Mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Realm {
    Server,
    Client,
    Shared,
}

impl Realm {
    /// Combines two realms: a specific realm overrides `Shared`, and
    /// `Server` with `Client` is `Shared`.
    #[allow(dead_code)]
    pub fn combine(self, other: Realm) -> Realm {
        match (self, other) {
            (Realm::Shared, r) | (r, Realm::Shared) => r,
            (a, b) if a == b => a,
            _ => Realm::Shared,
        }
    }

    /// The other side: `Server` and `Client` swap, `Shared` stays.
    pub fn opposite(self) -> Realm {
        match self {
            Realm::Server => Realm::Client,
            Realm::Client => Realm::Server,
            Realm::Shared => Realm::Shared,
        }
    }

    /// Parses `server`, `client` or `shared`, ignoring case.
    pub fn parse(s: &str) -> Option<Realm> {
        match s.trim().to_ascii_lowercase().as_str() {
            "server" => Some(Realm::Server),
            "client" => Some(Realm::Client),
            "shared" => Some(Realm::Shared),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Param {
    pub name: String,
    pub default: Option<String>,
    pub ty: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Return {
    pub ty: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variant {
    pub signature: String,
    pub description: String,
    pub params: Vec<Param>,
    /// Indented `@return` lines under the `@variant`.
    pub returns: Vec<Return>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Warning {
    pub label: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocBlock {
    /// Markdown description, tags removed.
    pub description: String,
    pub params: Vec<Param>,
    /// Top-level `@return` values; those indented under a `@variant` are in
    /// `Variant::returns`.
    pub returns: Vec<Return>,
    pub variants: Vec<Variant>,
    pub see: Vec<String>,
    pub warnings: Vec<Warning>,
    pub deprecation: Option<String>,
    pub deprecation_version: Option<String>,
    pub aliases: Vec<String>,
    pub category: Option<String>,
    /// Explicit `@realm`.
    pub realm: Option<Realm>,
    /// `@module [Name]`: the module a file doc documents, or the name of the
    /// object a local table defines.
    pub module: Option<String>,
    pub ignore: bool,
    /// Tags this parser does not know about, kept as (name, text).
    pub other: Vec<(String, String)>,
}

impl DocBlock {
    pub fn is_deprecated(&self) -> bool {
        self.deprecation.is_some() || self.deprecation_version.is_some()
    }

    pub fn is_internal(&self) -> bool {
        self.warnings.iter().any(|w| w.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case("internal")))
    }

    /// First sentence of the first paragraph of the description.
    pub fn summary(&self) -> String {
        first_sentence(&self.first_paragraph()).to_string()
    }

    /// First paragraph of the description, joined into one line.
    pub fn first_paragraph(&self) -> String {
        let mut out = Vec::new();
        for line in self.description.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("```") {
                if !out.is_empty() {
                    break;
                }
                if t.starts_with("```") {
                    break;
                }
                continue;
            }
            out.push(t);
        }
        out.join(" ")
    }
}

/// Cuts `text` after its first sentence: a `.`, `!` or `?` followed by
/// whitespace or the end, outside backticks and parentheses. A period
/// followed by a lowercase word (`e.g. this`) or ending an abbreviation such
/// as `e.g.` does not end the sentence.
fn first_sentence(text: &str) -> &str {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut in_code = false;
    let mut parens = 0usize;
    for (k, &(pos, c)) in chars.iter().enumerate() {
        match c {
            '`' => in_code = !in_code,
            '(' if !in_code => parens += 1,
            ')' if !in_code => parens = parens.saturating_sub(1),
            '.' | '!' | '?' if !in_code && parens == 0 => {
                if chars.get(k + 1).is_some_and(|&(_, n)| !n.is_whitespace()) {
                    continue;
                }
                let following = chars[k + 1..].iter().map(|&(_, n)| n).find(|n| !n.is_whitespace());
                if c == '.' && (following.is_some_and(char::is_lowercase) || is_abbreviation(&text[..pos])) {
                    continue;
                }
                return &text[..pos + c.len_utf8()];
            }
            _ => {}
        }
    }
    text
}

/// True when `before` (the text up to a period) ends with an abbreviation.
fn is_abbreviation(before: &str) -> bool {
    let word = before.rsplit(|c: char| !(c.is_alphanumeric() || c == '.')).next().unwrap_or("");
    ["e.g", "i.e", "vs", "cf"].iter().any(|a| word.eq_ignore_ascii_case(a))
}

/// Strips the comment markers from the raw comment texts (the text after
/// `--`) and returns the content lines, keeping relative indentation.
pub fn strip_markers(raw_lines: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = raw_lines
        .iter()
        .map(|l| {
            let mut s = l.as_str();
            // Extra leading dashes (`---`, `----`).
            while let Some(rest) = s.strip_prefix('-') {
                s = rest;
            }
            // One space of padding.
            s.strip_prefix(' ').unwrap_or(s).trim_end().to_string()
        })
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

enum Line {
    Text(String),
    /// A tag with its indentation, name and body (brackets included).
    Tag { indent: usize, name: String, body: String },
}

/// Groups the lines into text lines and tags, pulling bracketed tag bodies
/// that span several lines together.
fn classify(lines: &[String]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_fence = false;

    while i < lines.len() {
        let line = &lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && trimmed.starts_with('@') {
            let indent = line.len() - trimmed.len();
            let (name, body) = match trimmed[1..].find(char::is_whitespace) {
                Some(p) => (trimmed[1..1 + p].to_string(), trimmed[1 + p..].trim().to_string()),
                None => (trimmed[1..].to_string(), String::new()),
            };
            let mut body = body;
            // A `[` without a matching `]` continues on the following lines.
            while bracket_open(&body) && i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if next.starts_with('@') || next.is_empty() {
                    break;
                }
                body.push(' ');
                body.push_str(next);
                i += 1;
            }
            out.push(Line::Tag { indent, name, body });
        } else {
            out.push(Line::Text(line.clone()));
        }
        i += 1;
    }
    out
}

fn bracket_open(s: &str) -> bool {
    let opens = s.matches('[').count();
    let closes = s.matches(']').count();
    opens > closes
}

/// Splits `head [content]` into (head, Some(content)) or (text, None).
fn split_bracket(body: &str) -> (String, Option<String>) {
    match body.find('[') {
        Some(start) => {
            let head = body[..start].trim().to_string();
            let rest = &body[start + 1..];
            let content = match rest.rfind(']') {
                Some(end) => rest[..end].to_string(),
                None => rest.to_string(),
            };
            (head, Some(content.trim().to_string()))
        }
        None => (body.trim().to_string(), None),
    }
}

/// Splits `[Label] trailing text` into (Some(label), trailing) or (None, text).
fn split_leading_bracket(body: &str) -> (Option<String>, String) {
    let t = body.trim();
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return (Some(rest[..end].trim().to_string()), rest[end + 1..].trim().to_string());
        }
        return (Some(rest.trim().to_string()), String::new());
    }
    (None, t.to_string())
}

/// `Type description` -> (Some(Type), description).
fn split_type(content: &str) -> (Option<String>, String) {
    let t = content.trim();
    if t.is_empty() {
        return (None, String::new());
    }
    match t.find(char::is_whitespace) {
        Some(p) => (Some(t[..p].to_string()), t[p..].trim().to_string()),
        None => (Some(t.to_string()), String::new()),
    }
}

fn parse_param(body: &str) -> Param {
    let (head, content) = split_bracket(body);
    let (name, default) = match head.find('=') {
        Some(p) => (head[..p].trim().to_string(), Some(head[p + 1..].trim().to_string())),
        None => (head, None),
    };
    let (ty, description) = match content {
        Some(c) => split_type(&c),
        None => (None, String::new()),
    };
    Param { name, default: default.filter(|d| !d.is_empty()), ty, description }
}

/// Splits a return list such as `Boolean success, String error` on commas
/// that are followed by a type name (a capitalised word).
fn parse_returns(body: &str) -> Vec<Return> {
    let (_, content) = split_bracket(body);
    let content = match content {
        Some(c) => c,
        None => body.trim().to_string(),
    };
    if content.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut rest = content.as_str();
    while let Some(p) = rest.find(", ") {
        let after = &rest[p + 2..];
        if looks_like_type(after) {
            current.push_str(&rest[..p]);
            parts.push(current.trim().to_string());
            current = String::new();
            rest = after;
        } else {
            current.push_str(&rest[..p + 2]);
            rest = after;
        }
    }
    current.push_str(rest);
    parts.push(current.trim().to_string());

    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (ty, description) = split_type(p);
            Return { ty, description }
        })
        .collect()
}

fn looks_like_type(s: &str) -> bool {
    let word: String = s.chars().take_while(|c| !c.is_whitespace()).collect();
    let mut chars = word.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '<' | '>' | '/' | '(' | ')' | ',' | '|')),
        _ => false,
    }
}

/// Parses already stripped content lines.
pub fn parse_lines(lines: &[String]) -> DocBlock {
    let mut doc = DocBlock::default();
    let items = classify(lines);

    let mut description: Vec<String> = Vec::new();
    // Text lines since the last tag or blank line, used as the description
    // of a `@variant` that follows them.
    let mut pending: Vec<String> = Vec::new();
    let mut pending_has_first_line = false;
    let mut seen_text = false;
    let mut in_fence = false;
    // Index into doc.variants while nested `@param`s belong to a variant.
    let mut current_variant: Option<(usize, usize)> = None;

    let flush = |pending: &mut Vec<String>, description: &mut Vec<String>| {
        description.append(pending);
    };

    for item in items {
        match item {
            Line::Text(text) => {
                let trimmed = text.trim();
                if trimmed.starts_with("```") {
                    in_fence = !in_fence;
                }
                if in_fence || trimmed.starts_with("```") {
                    flush(&mut pending, &mut description);
                    description.push(text);
                    pending_has_first_line = false;
                    continue;
                }
                if trimmed.is_empty() {
                    flush(&mut pending, &mut description);
                    description.push(String::new());
                    pending_has_first_line = false;
                    continue;
                }
                if !seen_text {
                    seen_text = true;
                    pending_has_first_line = true;
                }
                // Nested text under a variant is still description.
                pending.push(text);
            }
            Line::Tag { indent, name, body } => {
                let lname = name.to_ascii_lowercase();
                // A nested @param or @return belongs to the open variant.
                if let Some((idx, vindent)) = current_variant
                    && indent > vindent
                {
                    match lname.as_str() {
                        "param" => {
                            doc.variants[idx].params.push(parse_param(&body));
                            continue;
                        }
                        "return" | "returns" => {
                            doc.variants[idx].returns.extend(parse_returns(&body));
                            continue;
                        }
                        _ => {}
                    }
                }
                if lname != "variant" {
                    flush(&mut pending, &mut description);
                    pending_has_first_line = false;
                    if indent == 0 || current_variant.is_none() {
                        current_variant = None;
                    }
                }
                match lname.as_str() {
                    "param" => doc.params.push(parse_param(&body)),
                    "return" | "returns" => doc.returns.extend(parse_returns(&body)),
                    "variant" => {
                        // The block's first line is the summary; any text after
                        // it (up to this tag) describes the variant.
                        if pending_has_first_line && !pending.is_empty() {
                            description.push(pending.remove(0));
                        }
                        let desc = pending.join(" ");
                        pending.clear();
                        pending_has_first_line = false;
                        doc.variants.push(Variant { signature: body.trim().to_string(), description: desc.trim().to_string(), params: Vec::new(), returns: Vec::new() });
                        current_variant = Some((doc.variants.len() - 1, indent));
                    }
                    "see" => {
                        let (_, content) = split_bracket(&body);
                        let target = content.unwrap_or_else(|| body.trim().to_string());
                        if !target.is_empty() {
                            doc.see.push(target);
                        }
                    }
                    "warning" => {
                        let (label, text) = split_leading_bracket(&body);
                        doc.warnings.push(Warning { label, text });
                    }
                    "deprecation" | "deprecated" => {
                        let (label, text) = split_leading_bracket(&body);
                        let reason = match label {
                            Some(l) if text.is_empty() => l,
                            Some(l) => format!("{l} {text}"),
                            None => text,
                        };
                        doc.deprecation = Some(reason);
                    }
                    "deprecation_version" | "since" => {
                        let (label, text) = split_leading_bracket(&body);
                        doc.deprecation_version = Some(label.unwrap_or(text));
                    }
                    "alias" => {
                        let (label, text) = split_leading_bracket(&body);
                        doc.aliases.push(label.unwrap_or(text));
                    }
                    "category" => {
                        let (label, text) = split_leading_bracket(&body);
                        doc.category = Some(label.unwrap_or(text));
                    }
                    "realm" => {
                        let (label, text) = split_leading_bracket(&body);
                        match Realm::parse(&label.unwrap_or(text)) {
                            Some(realm) => doc.realm = Some(realm),
                            None => doc.other.push((name, body)),
                        }
                    }
                    "module" => {
                        let (label, text) = split_leading_bracket(&body);
                        let module = label.unwrap_or(text);
                        if !module.is_empty() {
                            doc.module = Some(module);
                        }
                    }
                    "ignore" => doc.ignore = true,
                    _ => doc.other.push((name, body)),
                }
            }
        }
    }
    flush(&mut pending, &mut description);

    // Collapse runs of blank lines and trim.
    let mut out: Vec<String> = Vec::new();
    for l in description {
        if l.trim().is_empty() && out.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        out.push(l);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    doc.description = out.join("\n");
    doc
}

/// Parses raw comment texts (the text after `--` of each line).
pub fn parse(raw_lines: &[String]) -> DocBlock {
    parse_lines(&strip_markers(raw_lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(s: &str) -> Vec<String> {
        s.lines().map(|l| l.trim_start().trim_start_matches("--").to_string()).collect()
    }

    #[test]
    fn parses_demo_block() {
        let doc = parse(&raw(
            "--- Demo all that FDoc has to offer.
--
-- Triple ticks will be automatically recognized as an example.
--  ```
--  -- Some example. This comment will appear in the example.
--  test(123, player, { test = true })
--  ```
--
-- You can continue writing description after examples.
--
-- @warning [Internal] Some custom message.
-- @deprecation [Reason.]
-- @deprecation_version [0.8.0]
-- @param a=Default Value or Description [Number Some number]
-- @param b [Object Some object]
-- @return [Foo blank foo object, Number one hundred]
--
-- You can also refer to other functions (both variations are valid)
-- @see other_function
-- @see [MyClass#method]",
        ));
        assert_eq!(doc.summary(), "Demo all that FDoc has to offer.");
        assert!(doc.description.contains("```\n -- Some example."), "{}", doc.description);
        assert!(doc.description.ends_with("(both variations are valid)"));
        assert_eq!(doc.params.len(), 2);
        assert_eq!(doc.params[0].name, "a");
        assert_eq!(doc.params[0].default.as_deref(), Some("Default Value or Description"));
        assert_eq!(doc.params[0].ty.as_deref(), Some("Number"));
        assert_eq!(doc.params[0].description, "Some number");
        assert_eq!(doc.params[1].default, None);
        assert_eq!(doc.returns.len(), 2);
        assert_eq!(doc.returns[0].ty.as_deref(), Some("Foo"));
        assert_eq!(doc.returns[1].description, "one hundred");
        assert_eq!(doc.see, vec!["other_function", "MyClass#method"]);
        assert_eq!(doc.warnings[0].label.as_deref(), Some("Internal"));
        assert_eq!(doc.warnings[0].text, "Some custom message.");
        assert_eq!(doc.deprecation.as_deref(), Some("Reason."));
        assert_eq!(doc.deprecation_version.as_deref(), Some("0.8.0"));
        assert!(doc.is_internal() && doc.is_deprecated());
    }

    #[test]
    fn parses_variants_with_descriptions() {
        let doc = parse(&raw(
            "--- Get the items table from a certain inventory or from all of them.
-- Will return items from the specified inventory.
-- @variant player_meta:get_items(inv_type)
--   @param inv_type [String]
-- Will return items from all the inventories that the player has.
-- @variant player_meta:get_items()
-- @return [List<Item> items]",
        ));
        assert_eq!(doc.description, "Get the items table from a certain inventory or from all of them.");
        assert_eq!(doc.variants.len(), 2);
        assert_eq!(doc.variants[0].description, "Will return items from the specified inventory.");
        assert_eq!(doc.variants[0].params[0].name, "inv_type");
        assert_eq!(doc.variants[1].description, "Will return items from all the inventories that the player has.");
        assert!(doc.variants[1].params.is_empty());
        assert_eq!(doc.returns[0].ty.as_deref(), Some("List<Item>"));
        assert!(doc.params.is_empty());
    }

    #[test]
    fn joins_multiline_brackets_and_keeps_lowercase_commas() {
        let doc = parse(&raw(
            "--- Picks up.
-- @return [Boolean true if an object was picked up, false otherwise; nil if the player
--   is not valid]
-- @param data [Map mode definition: title, area_type and the optional
--   functions OnLeftClick(mode, tool, trace)]",
        ));
        assert_eq!(doc.returns.len(), 1);
        assert_eq!(doc.returns[0].description, "true if an object was picked up, false otherwise; nil if the player is not valid");
        assert_eq!(doc.params[0].description, "mode definition: title, area_type and the optional functions OnLeftClick(mode, tool, trace)");
    }

    #[test]
    fn parses_standalone_tags() {
        let doc = parse(&raw("--- @category [Query Engine]\n-- Provides utility functions."));
        assert_eq!(doc.category.as_deref(), Some("Query Engine"));
        assert_eq!(doc.description, "Provides utility functions.");
        let doc = parse(&raw("--- @ignore"));
        assert!(doc.ignore);
        let doc = parse(&raw("--- Determines if installed.\n-- @alias [Package.present]\n-- @alias [Package.is_installed]"));
        assert_eq!(doc.aliases, vec!["Package.present", "Package.is_installed"]);
        let doc = parse(&raw("--- @warning [Internal]\n-- Creates the player's default inventories."));
        assert_eq!(doc.description, "Creates the player's default inventories.");
        assert!(doc.is_internal());
    }

    #[test]
    fn summary_is_the_first_sentence() {
        let summary = |s: &str| parse(&raw(s)).summary();
        assert_eq!(summary("--- Does x. Then y."), "Does x.");
        assert_eq!(summary("--- Calls `a.b` then stops. More."), "Calls `a.b` then stops.");
        assert_eq!(summary("--- Runs e.g. the thing. More."), "Runs e.g. the thing.");
        assert_eq!(summary("--- Runs a task, e.g. 'migrate', in sync. More."), "Runs a task, e.g. 'migrate', in sync.");
        assert_eq!(summary("--- A single sentence without a period"), "A single sentence without a period");
        assert_eq!(summary("--- Wraps the call (see x. y) first! Then more."), "Wraps the call (see x. y) first!");
        assert_eq!(summary("--- Is it valid?\n-- Second line.\n--\n-- Second paragraph."), "Is it valid?");
        assert_eq!(summary("--- Spans\n-- two lines. Rest."), "Spans two lines.");
        assert_eq!(summary("--- Version 1.5 of `x`."), "Version 1.5 of `x`.");
    }

    #[test]
    fn indented_returns_belong_to_variants() {
        let doc = parse(&raw(
            "--- Finds things.
-- @variant find(id)
--   @param id [Number]
--   @return [Item the item]
-- @variant find()
--   @return [List<Item> all items]
-- @return [Boolean found]",
        ));
        assert_eq!(doc.variants[0].returns.len(), 1);
        assert_eq!(doc.variants[0].returns[0].ty.as_deref(), Some("Item"));
        assert_eq!(doc.variants[1].returns[0].ty.as_deref(), Some("List<Item>"));
        assert!(doc.variants[1].params.is_empty());
        assert_eq!(doc.returns.len(), 1);
        assert_eq!(doc.returns[0].ty.as_deref(), Some("Boolean"));
    }

    #[test]
    fn parses_realm_and_module() {
        let doc = parse(&raw("--- Server side.\n-- @realm [Server]"));
        assert_eq!(doc.realm, Some(Realm::Server));
        let doc = parse(&raw("--- Client side.\n-- @realm client"));
        assert_eq!(doc.realm, Some(Realm::Client));
        let doc = parse(&raw("--- Odd.\n-- @realm [moon]"));
        assert_eq!(doc.realm, None);
        assert_eq!(doc.other, vec![("realm".to_string(), "[moon]".to_string())]);
        let doc = parse(&raw("--- Currencies.\n-- @module [cw.currency]"));
        assert_eq!(doc.module.as_deref(), Some("cw.currency"));
        assert_eq!(doc.description, "Currencies.");
    }

    #[test]
    fn combines_realms() {
        assert_eq!(Realm::Shared.combine(Realm::Server), Realm::Server);
        assert_eq!(Realm::Client.combine(Realm::Shared), Realm::Client);
        assert_eq!(Realm::Server.combine(Realm::Client), Realm::Shared);
        assert_eq!(Realm::Server.combine(Realm::Server), Realm::Server);
        assert_eq!(Realm::Server.opposite(), Realm::Client);
        assert_eq!(Realm::Shared.opposite(), Realm::Shared);
    }
}
