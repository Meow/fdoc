//! fdoc — generates HTML documentation from the doc comments of a Lua
//! code base.

mod doc;
mod html;
mod lexer;
mod markdown;
mod model;
mod render;
mod scanner;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use model::BuildOptions;

const USAGE: &str = "\
fdoc — HTML documentation generator for Lua doc comments

Usage: fdoc [OPTIONS] <SOURCE_DIR>

Arguments:
  <SOURCE_DIR>            Directory to scan recursively for .lua files

Options:
  -o, --output <DIR>      Output directory (default: <SOURCE_DIR>/docs)
      --title <NAME>      Project name (default: from packagespec.lua, else the directory name)
      --source-url <URL>  Base URL that source paths are appended to for \"view source\" links
      --exclude <NAME>    Directory name to skip; repeatable (default: docs, .git)
      --documented-only   Only include functions that have a doc comment
      --clean             Delete the output directory before generating
  -q, --quiet             Only print errors
  -h, --help              Print this help
  -V, --version           Print the version
";

struct Args {
    source: PathBuf,
    output: Option<PathBuf>,
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
    let mut out = Args { source: PathBuf::new(), output: None, title: None, source_url: None, excludes: vec!["docs".into(), ".git".into()], documented_only: false, clean: false, quiet: false };

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

/// Collects the `.lua` files below `root`, sorted, as paths relative to it.
fn collect_lua_files(root: &Path, excludes: &[String], skip: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let file_type = entry.file_type().map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if file_type.is_dir() {
                if name.starts_with('.') || excludes.contains(&name) || skip.is_some_and(|s| same_path(s, &path)) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && name.ends_with(".lua") {
                files.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
    }
    files.sort();
    Ok(files)
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

    let files = collect_lua_files(source, &args.excludes, Some(&output))?;
    if files.is_empty() {
        return Err(format!("no .lua files found in {}", source.display()));
    }

    let mut scanned = Vec::with_capacity(files.len());
    for rel in &files {
        let path = source.join(rel);
        let text = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&text);
        let rel = rel.to_string_lossy().replace('\\', "/");
        scanned.push((rel, scanner::scan(&text)));
    }

    let dir_name = source.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let project = model::build(scanned, BuildOptions { title: args.title.as_deref(), fallback_title: dir_name.as_deref(), documented_only: args.documented_only, source_url: args.source_url.as_deref() });

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
        println!("Scanned {} files: {modules} modules, {functions} functions ({documented} documented).", files.len());
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
