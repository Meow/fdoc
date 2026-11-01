//! fdoc as a library: generates the HTML documentation pages, or the JSON
//! document of the same model, from the doc comments of a Lua code base,
//! with its Lumen templates and NL files, held in memory.
//!
//! The command line tool walks a directory and calls [`generate_with`];
//! the WebAssembly build (`src/wasm.rs`) parses a JSON [`Input`] and calls
//! [`generate`]. Nothing in here touches files, threads or clocks.
//!
//! ```
//! let input = fdoc::Input {
//!     title: Some("Cable".into()),
//!     files: vec![fdoc::File::new("lib/cable.lua", "--- Sends a message.\nfunction cable.send(msg) end\n")],
//!     ..fdoc::Input::default()
//! };
//! let output = fdoc::generate(input).unwrap();
//! assert!(output.pages.iter().any(|p| p.path == "index.html"));
//! ```

mod doc;
mod html;
mod json;
mod layout;
mod lexer;
mod markdown;
mod model;
mod nl;
mod render;
mod scanner;
#[cfg(target_arch = "wasm32")]
mod wasm;

use std::collections::{HashMap, HashSet};
use std::fmt;

use layout::{GroupMeta, Layout};
use lexer::Syntax;
use model::{BuildOptions, PlacedFile};

pub use layout::Config;
pub use render::Page;

/// The crate version, `fdoc --version` without the name.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A file given to [`generate`]: a path relative to the source root, `/`
/// separated, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub text: String,
}

impl File {
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> File {
        File { path: path.into(), text: text.into() }
    }
}

/// What fdoc writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Format {
    /// The HTML pages, with their assets.
    #[default]
    Html,
    /// One JSON document of the whole model, as the page `fdoc.json`.
    Json,
}

/// What to document.
///
/// `files` are the files of the source tree, with paths relative to its
/// root. The ones with a source file name (`.lua`, `.lumen`, `.nl`,
/// `.nlua`, but not `.d.nl`) are documented if a group of the layout takes
/// them. Every file, documented or not, is also where fdoc looks for the
/// metadata it reads beside the sources: the `packagespec.lua` of each
/// directory (the project title, version, summary and description from the
/// root one, and which packages are `serverside`) and the `plugin.yml`,
/// `plugin.yaml` or `plugin.ini` of each group's directory. So a caller that
/// wants that metadata passes those files too; a `packagespec.lua` is a
/// source file and is documented like any other.
///
/// Excludes are the caller's business: `files` is taken as given. The
/// layout and the project settings come from `config` (what a `.fdoc.yml`
/// holds, parsed with [`Config::from_yaml`]); its `exclude` list is not
/// applied here, and its `warnings` are the caller's to show. The explicit
/// `title`, `version` and `source_url` override the config's, which
/// override the root `packagespec.lua`.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Project name. Default: the config's `title`, else the root
    /// packagespec's `name`, else `fallback_title`, else "Documentation".
    pub title: Option<String>,
    /// Project version. Default: the config's, else the root packagespec's.
    pub version: Option<String>,
    /// Base URL that source paths are appended to for "view source" links.
    pub source_url: Option<String>,
    pub files: Vec<File>,
    pub config: Option<Config>,
    /// The name used when nothing else names the project, such as the name
    /// of the source directory.
    pub fallback_title: Option<String>,
    /// Only include functions that have a doc comment (also set by the
    /// config's `documented_only`).
    pub documented_only: bool,
    pub format: Format,
}

/// Counts for a summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Source files that were scanned (the ones a group of the layout took).
    pub scanned: usize,
    /// Source files that match no group of the layout.
    pub skipped: usize,
    pub modules: usize,
    pub functions: usize,
    /// Functions with a doc comment.
    pub documented: usize,
}

/// The generated documentation.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// The pages with paths relative to the output root, `/` separated:
    /// the HTML pages and their assets, or the one `fdoc.json`.
    pub pages: Vec<Page>,
    /// Problems in the files that did not stop generation, as
    /// `<path>: <message>` (markup nested too deep, ...). The config's own
    /// warnings are not repeated here, and source files that match no group
    /// of the layout are counted in `stats.skipped`.
    pub warnings: Vec<String>,
    pub stats: Stats,
}

/// Why nothing was generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `files` holds no source file.
    NoSourceFiles,
    /// None of the `total` source files is in a group of the layout.
    NoFilesInLayout { total: usize },
    /// A metadata file (`plugin.yml`) at `path` does not parse.
    Metadata { path: String, message: String },
    /// The metadata reader of [`generate_with`] failed; the message is its
    /// own.
    Read(String),
    /// A path in `files` is absolute, empty or has a `..` component.
    InvalidPath(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSourceFiles => write!(f, "no .lua, .lumen, .nl or .nlua files in the input"),
            Error::NoFilesInLayout { total } => write!(f, "none of the {total} source files matches a group of the layout"),
            Error::Metadata { path, message } => write!(f, "{path}: {message}"),
            Error::Read(message) => f.write_str(message),
            Error::InvalidPath(path) => write!(f, "invalid file path `{path}`: paths are relative, without `..` components"),
        }
    }
}

impl std::error::Error for Error {}

/// True for the name of a file fdoc reads: a Lua file, a Lumen template or
/// an NL file (`.nl`, `.nlua`), but not an NL declaration file (`.d.nl`).
pub fn is_source_file(name: &str) -> bool {
    [".lua", ".lumen", ".nl", ".nlua"].iter().any(|ext| name.ends_with(ext)) && !name.ends_with(".d.nl")
}

/// A test for whether a group of the config's layout (the built-in one
/// without a `sections` entry) takes the source file at a path. Files for
/// which it is false are not scanned, so a caller can leave them out of the
/// input.
pub fn layout_filter(config: Option<&Config>) -> impl Fn(&str) -> bool + use<> {
    let layout = config.and_then(|c| c.layout.clone()).unwrap_or_else(|| Layout::default_for(""));
    move |path| layout.classify(path).is_some()
}

/// Generates the documentation of the files in `input`, reading the
/// metadata files from `input.files` (see [`Input`]).
pub fn generate(input: Input) -> Result<Output, Error> {
    let by_path: HashMap<String, String> = input.files.iter().map(|f| (f.path.clone(), f.text.clone())).collect();
    generate_with(input, &mut |path: &str| Ok(by_path.get(path).cloned()))
}

/// [`generate`] with the metadata files (`packagespec.lua`, `plugin.yml`,
/// `plugin.yaml`, `plugin.ini`) read through `read` instead of looked up in
/// `input.files`: it gets a path relative to the source root and returns
/// the file's text, `None` when there is no such file, or an error message
/// that becomes [`Error::Read`]. The command line tool reads them from disk
/// this way, including the specs its excludes leave out of the sources.
pub fn generate_with(input: Input, read: &mut dyn FnMut(&str) -> Result<Option<String>, String>) -> Result<Output, Error> {
    let config = input.config.unwrap_or_default();
    let mut sources: Vec<File> = Vec::with_capacity(input.files.len());
    for f in input.files {
        let path = normalize_path(&f.path)?;
        if is_source_file(path.rsplit('/').next().unwrap_or(&path)) {
            sources.push(File { path, text: f.text });
        }
    }
    // The output must not depend on the order of `files`: sort by path
    // (bytewise, like the directory walk of the command line tool) and keep
    // the last of several files with one path.
    sources.reverse();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    sources.dedup_by(|later, earlier| later.path == earlier.path);
    if sources.is_empty() {
        return Err(Error::NoSourceFiles);
    }
    let title = input.title.as_deref().or(config.title.as_deref());
    let version = input.version.as_deref().or(config.version.as_deref());
    let source_url = input.source_url.as_deref().or(config.source_url.as_deref());
    let documented_only = input.documented_only || config.documented_only.unwrap_or(false);

    // Only the files inside a group of the layout are scanned (plus the
    // `packagespec.lua` files `read_packages` looks at). The built-in layout
    // takes its section title from the root packagespec, which is only
    // known after scanning; the title does not affect where files go.
    let total = sources.len();
    let configured = config.layout.is_some();
    let mut layout = config.layout.clone().unwrap_or_else(|| Layout::default_for(""));
    let mut scanned: Vec<PlacedFile> = Vec::with_capacity(sources.len());
    for file in sources {
        if let Some(place) = layout.classify(&file.path) {
            let scan = scanner::scan_file(&file.text, &file.path);
            scanned.push((file.path, place, scan));
        }
    }
    if scanned.is_empty() {
        return Err(Error::NoFilesInLayout { total });
    }
    let skipped = total - scanned.len();
    let warnings: Vec<String> = scanned.iter().flat_map(|(path, _, scan)| scan.warnings.iter().map(move |w| format!("{path}: {w}"))).collect();
    let scanned_count = scanned.len();

    let fallback = input.fallback_title.as_deref();
    if !configured {
        layout = Layout::default_for(&model::project_title(&scanned, title, fallback));
    }
    let group_meta = read_group_meta(&scanned, read)?;
    let packages = read_packages(&scanned, read)?;
    let project = model::build(
        scanned,
        BuildOptions {
            title,
            fallback_title: fallback,
            version,
            summary: config.summary.as_deref(),
            description: config.description.as_deref(),
            documented_only,
            source_url,
            packages: Some(&packages),
            ..BuildOptions::new(&layout, &group_meta)
        },
    );

    let stats = Stats {
        scanned: scanned_count,
        skipped,
        modules: project.all_modules().count(),
        functions: project.all_modules().map(|(_, m)| m.functions.len()).sum(),
        documented: project.all_modules().map(|(_, m)| m.functions.iter().filter(|f| f.doc.is_some()).count()).sum(),
    };
    let pages = match input.format {
        Format::Html => render::render_all(&project),
        Format::Json => vec![Page { path: "fdoc.json".into(), content: json::render(&project) }],
    };
    Ok(Output { pages, warnings, stats })
}

/// A file path relative to the source root with `/` separators and no
/// empty or `.` components; `..` components and absolute paths are refused,
/// so that no page path can leave the output directory.
fn normalize_path(path: &str) -> Result<String, Error> {
    let comps: Vec<&str> = path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
    let absolute = path.starts_with('/') || path.starts_with('\\') || comps.first().is_some_and(|c| c.len() == 2 && c.ends_with(':'));
    if comps.is_empty() || absolute || comps.contains(&"..") {
        return Err(Error::InvalidPath(path.to_string()));
    }
    Ok(comps.join("/"))
}

/// `dir/name`, or `name` in the root.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// Reads the `plugin.yml` (else `plugin.yaml`, else `plugin.ini`) in each
/// group's source directory, by that directory.
fn read_group_meta(files: &[PlacedFile], read: &mut dyn FnMut(&str) -> Result<Option<String>, String>) -> Result<HashMap<String, GroupMeta>, Error> {
    let mut meta = HashMap::new();
    let mut seen = HashSet::new();
    for (_, place, _) in files {
        if !seen.insert(place.source_dir.as_str()) {
            continue;
        }
        let mut parsed = None;
        for name in ["plugin.yml", "plugin.yaml"] {
            let path = join(&place.source_dir, name);
            if let Some(text) = read(&path).map_err(Error::Read)? {
                parsed = Some(layout::parse_plugin_yml(&text).map_err(|message| Error::Metadata { path, message })?);
                break;
            }
        }
        if parsed.is_none()
            && let Some(text) = read(&join(&place.source_dir, "plugin.ini")).map_err(Error::Read)?
        {
            parsed = Some(layout::parse_plugin_ini(&text));
        }
        if let Some(parsed) = parsed {
            meta.insert(place.source_dir.clone(), parsed);
        }
    }
    Ok(meta)
}

/// Reads whether each `packagespec.lua` in the directories of the source
/// files and above them, up to the source root, sets `serverside`, even
/// where the layout or the excludes leave the spec out. Keyed by directory,
/// empty for the root.
fn read_packages(files: &[PlacedFile], read: &mut dyn FnMut(&str) -> Result<Option<String>, String>) -> Result<HashMap<String, bool>, Error> {
    let mut packages = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for (path, _, _) in files {
        let mut dir = path.as_str();
        while !dir.is_empty() {
            dir = dir.rfind('/').map_or("", |p| &dir[..p]);
            if !seen.insert(dir) {
                break;
            }
            if let Some(text) = read(&join(dir, "packagespec.lua")).map_err(Error::Read)? {
                let scan = scanner::scan_as(&text, Syntax::Lua);
                packages.insert(dir.to_string(), scan.serverside);
            }
        }
    }
    Ok(packages)
}

/// Parses the JSON form of an [`Input`]:
/// `{ "title", "version", "source_url", "files": [{ "path", "text" }] }`,
/// every member but `files` optional or `null`, plus the optional
/// extensions `"config"` (the text of a `.fdoc.yml`), `"format"` (`"html"`
/// or `"json"`) and `"documented_only"` (a boolean). Returns the input and
/// the config's warnings.
pub fn input_from_json(text: &str) -> Result<(Input, Vec<String>), String> {
    let value = json::parse(text)?;
    let json::Value::Object(members) = value else { return Err("the input must be a JSON object".into()) };
    let mut input = Input::default();
    let mut warnings = Vec::new();
    let string = |key: &str, v: json::Value| match v {
        json::Value::Null => Ok(None),
        json::Value::Str(s) => Ok(Some(s)),
        _ => Err(format!("`{key}` must be a string")),
    };
    let mut have_files = false;
    for (key, v) in members {
        match key.as_str() {
            "title" => input.title = string(&key, v)?,
            "version" => input.version = string(&key, v)?,
            "source_url" => input.source_url = string(&key, v)?,
            "fallback_title" => input.fallback_title = string(&key, v)?,
            "config" => {
                if let Some(yaml) = string(&key, v)? {
                    let config = Config::from_yaml(&yaml).map_err(|e| format!("config: {e}"))?;
                    warnings.extend(config.warnings.iter().map(|w| format!("config: {w}")));
                    input.config = Some(config);
                }
            }
            "format" => {
                input.format = match string(&key, v)?.as_deref() {
                    None | Some("html") => Format::Html,
                    Some("json") => Format::Json,
                    Some(other) => return Err(format!("unknown format: {other} (expected html or json)")),
                }
            }
            "documented_only" => {
                input.documented_only = match v {
                    json::Value::Null => false,
                    json::Value::Bool(b) => b,
                    _ => return Err("`documented_only` must be a boolean".into()),
                }
            }
            "files" => {
                have_files = true;
                let json::Value::Array(items) = v else { return Err("`files` must be an array".into()) };
                for (i, item) in items.into_iter().enumerate() {
                    let json::Value::Object(fields) = item else { return Err(format!("files[{i}] must be an object")) };
                    let (mut path, mut text) = (None, None);
                    for (k, v) in fields {
                        match (k.as_str(), v) {
                            ("path", json::Value::Str(s)) => path = Some(s),
                            ("text", json::Value::Str(s)) => text = Some(s),
                            ("path" | "text", _) => return Err(format!("files[{i}].{k} must be a string")),
                            _ => {}
                        }
                    }
                    match (path, text) {
                        (Some(path), Some(text)) => input.files.push(File { path, text }),
                        _ => return Err(format!("files[{i}] needs a `path` and a `text`")),
                    }
                }
            }
            _ => {}
        }
    }
    if !have_files {
        return Err("the input has no `files`".into());
    }
    Ok((input, warnings))
}

/// The JSON form of an [`Output`]: `{ "pages": [{ "path", "content" }],
/// "warnings": [...] }`, in ASCII.
pub fn output_to_json(output: &Output, extra_warnings: &[String]) -> String {
    let mut out = String::with_capacity(output.pages.iter().map(|p| p.content.len() + p.path.len() + 32).sum::<usize>() + 64);
    out.push_str("{\"pages\":[");
    for (i, page) in output.pages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"path\":");
        json::escape(&page.path, &mut out);
        out.push_str(",\"content\":");
        json::escape(&page.content, &mut out);
        out.push('}');
    }
    out.push_str("],\"warnings\":[");
    let skipped = (output.stats.skipped > 0).then(|| format!("skipped {} files that match no group of the layout", output.stats.skipped));
    for (i, w) in extra_warnings.iter().chain(&output.warnings).chain(&skipped).enumerate() {
        if i > 0 {
            out.push(',');
        }
        json::escape(w, &mut out);
    }
    out.push_str("]}");
    out
}

/// `{ "error": "<message>" }`.
pub fn error_to_json(message: &str) -> String {
    let mut out = String::from("{\"error\":");
    json::escape(message, &mut out);
    out.push('}');
    out
}

/// Parses JSON input and generates, answering with the JSON output or the
/// JSON error; what the WebAssembly export does.
pub fn generate_json(input: &str) -> String {
    match input_from_json(input) {
        Err(e) => error_to_json(&format!("invalid input: {e}")),
        Ok((input, warnings)) => match generate(input) {
            Ok(output) => output_to_json(&output, &warnings),
            Err(e) => error_to_json(&e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> String {
        format!("Package:describe(function(s)\n  s.name = '{name}'\n  s.version = '1.0.0'\nend)\n")
    }

    #[test]
    fn generates_pages_from_memory() {
        let input = Input {
            files: vec![File::new("packagespec.lua", spec("Cable")), File::new("lib/cable.lua", "--- Sends.\nfunction cable.send(msg) end\n"), File::new("README.md", "not a source")],
            source_url: Some("https://example.com/src/".into()),
            ..Input::default()
        };
        let out = generate(input).unwrap();
        assert!(out.pages.iter().any(|p| p.path == "index.html" && p.content.contains("Cable")));
        assert!(out.pages.iter().any(|p| p.path == "assets/app.js"));
        assert_eq!(out.stats.scanned, 2);
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn explicit_title_wins_and_json_format() {
        let input = Input { title: Some("Other".into()), files: vec![File::new("packagespec.lua", spec("Cable"))], format: Format::Json, ..Input::default() };
        let out = generate(input).unwrap();
        assert_eq!(out.pages.len(), 1);
        assert_eq!(out.pages[0].path, "fdoc.json");
        assert!(out.pages[0].content.contains("\"title\": \"Other\""));
    }

    #[test]
    fn errors() {
        assert_eq!(generate(Input { files: vec![File::new("a.md", "")], ..Input::default() }).unwrap_err(), Error::NoSourceFiles);
        let config = Config::from_yaml("sections:\n  - title: X\n    groups:\n      - path: only\n").unwrap();
        let err = generate(Input { files: vec![File::new("a.lua", "")], config: Some(config), ..Input::default() }).unwrap_err();
        assert_eq!(err, Error::NoFilesInLayout { total: 1 });
    }

    #[test]
    fn plugin_metadata_from_files() {
        let files = vec![File::new("plugins/foo/sh_plugin.lua", "--- Hi.\nfunction PLUGIN:Hi() end\n"), File::new("plugins/foo/plugin.yml", "name: Fancy Foo\n")];
        let out = generate(Input { files, ..Input::default() }).unwrap();
        assert!(out.pages.iter().any(|p| p.content.contains("Fancy Foo")));
        let files = vec![File::new("plugins/foo/sh_plugin.lua", ""), File::new("plugins/foo/plugin.yml", "name: [unclosed\n")];
        assert!(matches!(generate(Input { files, ..Input::default() }), Err(Error::Metadata { path, .. }) if path == "plugins/foo/plugin.yml"));
    }

    #[test]
    fn output_does_not_depend_on_file_order() {
        let files = vec![
            File::new("packagespec.lua", spec("Cable")),
            File::new("lib/b.lua", "--- B.\nfunction cable.b() end\n"),
            File::new("lib/a.lua", "--- A.\nfunction cable.a() end\n"),
            File::new("lib/\u{e9}.lua", "--- E.\nfunction cable.e() end\n"),
            File::new("lib/z.lua", "--- Z.\nfunction cable.z() end\n"),
        ];
        let pages = |files: Vec<File>| generate(Input { files, ..Input::default() }).unwrap().pages;
        let forward = pages(files.clone());
        let mut reversed = files.clone();
        reversed.reverse();
        assert_eq!(forward, pages(reversed));
        let mut dotted = files;
        dotted[1].path = "./lib//b.lua".into();
        assert_eq!(forward, pages(dotted));
    }

    #[test]
    fn refuses_paths_that_leave_the_root() {
        for bad in ["../x.lua", "lib/../../x.lua", "/etc/x.lua", "C:/x.lua", "", "\\\\server\\x.lua"] {
            let err = generate(Input { files: vec![File::new(bad, "")], ..Input::default() }).unwrap_err();
            assert_eq!(err, Error::InvalidPath(bad.to_string()), "{bad}");
        }
    }

    #[test]
    fn deep_markup_is_a_warning_not_a_crash() {
        let depth = 15_000 / 3;
        let text = format!("return {}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        let out = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || generate(Input { files: vec![File::new("deep.lumen", text)], ..Input::default() }).unwrap())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].starts_with("deep.lumen: line 1: markup nests deeper than 256 elements"), "{:?}", out.warnings);
    }

    #[test]
    fn json_round_trip() {
        let out = generate_json(r#"{"title":"Cable","version":"1.0.0","source_url":null,"files":[{"path":"lib/a.lua","text":"--- Déjà.\nfunction a.b() end\n"}]}"#);
        assert!(out.starts_with("{\"pages\":[{\"path\":\"assets/style.css\""), "{}", &out[..80]);
        assert!(out.ends_with("\"warnings\":[]}"));
        assert!(json::parse(&out).is_ok());
        assert_eq!(generate_json("[]"), r#"{"error":"invalid input: the input must be a JSON object"}"#);
        assert_eq!(generate_json(r#"{"files":[]}"#), r#"{"error":"no .lua, .lumen, .nl or .nlua files in the input"}"#);
        assert!(generate_json("{\"files\":[").starts_with("{\"error\":\"invalid input: "));
    }
}
