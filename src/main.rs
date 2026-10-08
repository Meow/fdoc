//! fdoc — generates HTML documentation from the doc comments of a Lua
//! code base.

mod doc;
mod html;
mod layout;
mod lexer;
mod markdown;
mod model;
mod render;
mod scanner;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use layout::{Config, GroupMeta, Layout, Placement};
use model::{BuildOptions, PlacedFile};

const USAGE: &str = "\
fdoc — HTML documentation generator for Lua doc comments

Usage: fdoc [OPTIONS] <SOURCE_DIR>

Arguments:
  <SOURCE_DIR>            Directory to scan recursively for .lua files

Options:
  -o, --output <DIR>      Output directory (default: <SOURCE_DIR>/docs)
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
    let mut out = Args { source: PathBuf::new(), output: None, config: None, title: None, source_url: None, excludes: vec!["docs".into(), ".git".into()], documented_only: false, clean: false, quiet: false };

    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} requires a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("fdoc {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-o" | "--output" => out.output = Some(PathBuf::from(value("--output")?)),
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

/// Collects the `.lua` files below `root`, sorted, as `/` separated paths
/// relative to it.
fn collect_lua_files(root: &Path, excludes: &[String], skip: Option<&Path>) -> Result<Vec<String>, String> {
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
            } else if file_type.is_file() && name.ends_with(".lua") && !is_excluded(&rel, &name, false, excludes) {
                files.push(rel);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Reads the `plugin.ini` in each group's source directory, by that
/// directory.
fn read_group_meta(source: &Path, files: &[PlacedFile]) -> Result<HashMap<String, GroupMeta>, String> {
    let mut meta = HashMap::new();
    let mut seen = HashSet::new();
    for (_, place, _) in files {
        if !seen.insert(place.source_dir.as_str()) {
            continue;
        }
        let ini = source.join(&place.source_dir).join("plugin.ini");
        if ini.is_file() {
            let text = fs::read(&ini).map_err(|e| format!("cannot read {}: {e}", ini.display()))?;
            meta.insert(place.source_dir.clone(), layout::parse_plugin_ini(&String::from_utf8_lossy(&text)));
        }
    }
    Ok(meta)
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

    let (config_path, config) = load_config(&args)?;
    if !args.quiet {
        let name = config_path.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
        for warning in &config.warnings {
            eprintln!("warning: {name}: {warning}");
        }
    }
    let title = args.title.as_deref().or(config.title.as_deref());
    let source_url = args.source_url.as_deref().or(config.source_url.as_deref());
    let documented_only = args.documented_only || config.documented_only.unwrap_or(false);
    let excludes: Vec<String> = args.excludes.iter().chain(&config.exclude).cloned().collect();

    let files = collect_lua_files(source, &excludes, Some(&output))?;
    if files.is_empty() {
        return Err(format!("no .lua files found in {}", source.display()));
    }
    // Files outside every group of the layout are not read at all. The
    // built-in layout is titled once the root packagespec is read; the
    // title does not change where files are placed.
    let total = files.len();
    let configured = config.layout.is_some();
    let mut layout = config.layout.unwrap_or_else(|| Layout::default_for(""));
    let files: Vec<(String, Placement)> = files.into_iter().filter_map(|f| layout.classify(&f).map(|p| (f, p))).collect();
    if files.is_empty() {
        return Err(format!("none of the {total} .lua files in {} matches a group of the layout", source.display()));
    }
    let skipped = total - files.len();

    let mut scanned: Vec<PlacedFile> = Vec::with_capacity(files.len());
    for (rel, place) in files {
        let path = source.join(&rel);
        let text = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let scan = scanner::scan(&String::from_utf8_lossy(&text));
        scanned.push((rel, place, scan));
    }
    let scanned_count = scanned.len();

    let dir_name = source.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    if !configured {
        layout = Layout::default_for(&model::project_title(&scanned, title, dir_name.as_deref()));
    }
    let group_meta = read_group_meta(source, &scanned)?;
    let project = model::build(
        scanned,
        BuildOptions {
            title,
            fallback_title: dir_name.as_deref(),
            version: config.version.as_deref(),
            summary: config.summary.as_deref(),
            description: config.description.as_deref(),
            documented_only,
            source_url,
            ..BuildOptions::new(&layout, &group_meta)
        },
    );

    if args.clean && output.exists() {
        if same_path(&output, source) || source.canonicalize().map(|s| s.starts_with(output.canonicalize().unwrap_or_default())).unwrap_or(false) {
            return Err(format!("refusing to clean {}: it contains the source directory", output.display()));
        }
        fs::remove_dir_all(&output).map_err(|e| format!("cannot remove {}: {e}", output.display()))?;
    }

    let pages = render::render_all(&project);
    for page in &pages {
        let path = output.join(&page.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        fs::write(&path, &page.content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }

    if !args.quiet {
        let modules = project.all_modules().count();
        let functions: usize = project.all_modules().map(|(_, m)| m.functions.len()).sum();
        let documented: usize = project.all_modules().map(|(_, m)| m.functions.iter().filter(|f| f.doc.is_some()).count()).sum();
        println!("Scanned {scanned_count} files: {modules} modules, {functions} functions ({documented} documented).");
        if skipped > 0 {
            println!("Skipped {skipped} files that match no group of the layout.");
        }
        println!("Wrote {} pages to {}", pages.len(), output.display());
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
    fn root_relative_excludes_are_paths() {
        let excludes = vec!["./lib".to_string(), "./packagespec.lua".to_string(), "vendor/".to_string()];
        assert!(is_excluded("lib", "lib", true, &excludes));
        assert!(!is_excluded("gamemodes/a/lib", "lib", true, &excludes), "`./lib` is the root `lib` only");
        assert!(is_excluded("packagespec.lua", "packagespec.lua", false, &excludes), "a path can name a file");
        assert!(!is_excluded("packages/a/packagespec.lua", "packagespec.lua", false, &excludes));
        assert!(is_excluded("a/vendor", "vendor", true, &excludes), "a trailing `/` keeps a name");
    }
}
