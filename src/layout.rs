//! The documentation layout: which source files form which groups, and the
//! sidebar sections the groups are listed under. It is read from a
//! `.fdoc.yml` file; without one the built-in Flux layout is used.

use std::collections::HashSet;

use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

use crate::model::{file_stem, slugify};

/// One component of a group path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    /// `*`: any directory.
    Any,
    /// `*.lua` as the last component: any Lua file.
    AnyLua,
}

/// A `groups` entry of a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    /// The path as written, normalised; `.` is the whole source tree.
    pub path: String,
    /// Fixed group title.
    pub name: Option<String>,
    /// Core groups have no index page; their modules are listed directly
    /// under the section.
    pub core: bool,
    parts: Vec<Part>,
    /// Index in the section's `groups` of the first spec of this spec's
    /// family; see `Placement::family`.
    family: usize,
    /// Output directory. For paths with a `*`, the directory that the group
    /// slug is appended to.
    dir: String,
}

impl GroupSpec {
    /// Parses a group path; `dir` is assigned by the layout afterwards.
    pub fn new(path: &str, name: Option<String>, core: bool) -> Result<GroupSpec, String> {
        let mut parts = Vec::new();
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        for (i, c) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            parts.push(match *c {
                "*" => Part::Any,
                "*.lua" if last => Part::AnyLua,
                "*.lua" => return Err(format!("group path `{path}`: `*.lua` must be the last component")),
                ".." => return Err(format!("group path `{path}`: `..` is not allowed")),
                c if c.contains('*') => return Err(format!("group path `{path}`: only `*` and `*.lua` are supported as wildcards")),
                c => Part::Literal(c.to_string()),
            });
        }
        let path = if comps.is_empty() { ".".to_string() } else { comps.join("/") };
        Ok(GroupSpec { path, name, core, parts, family: 0, dir: String::new() })
    }

    pub fn is_pattern(&self) -> bool {
        self.parts.iter().any(|p| !matches!(p, Part::Literal(_)))
    }

    /// For a path with a wildcard, the path with a last `*.lua` read as `*`,
    /// so that `plugins/*` and `plugins/*.lua` share it.
    fn family_path(&self) -> Option<String> {
        self.is_pattern().then(|| match self.path.strip_suffix("*.lua") {
            Some(base) => format!("{base}*"),
            None => self.path.clone(),
        })
    }

    /// The literal components of the path joined with `-`, to tell the
    /// output directories of several wildcard paths in one section apart.
    fn literal_slug(&self) -> String {
        let literals: Vec<&str> = self.parts.iter().filter_map(|p| if let Part::Literal(l) = p { Some(l.as_str()) } else { None }).collect();
        if literals.is_empty() { "root".to_string() } else { slugify(&literals.join("-")) }
    }

    /// Paths with more literal components win; `.` loses to everything.
    fn priority(&self) -> i32 {
        if self.parts.is_empty() {
            return -1;
        }
        self.parts.iter().filter(|p| matches!(p, Part::Literal(_))).count() as i32
    }

    /// Matches the components of a file path; returns the group key and the
    /// group's source directory.
    fn matches(&self, comps: &[&str]) -> Option<(String, String)> {
        let n = self.parts.len();
        // A path as long as the pattern names a file, a longer one a file
        // below the directory the pattern names.
        let exact = comps.len() == n;
        if comps.len() < n || (exact && !matches!(self.parts.last(), Some(Part::AnyLua | Part::Literal(_)))) {
            return None;
        }
        let mut matched = Vec::new();
        for (part, comp) in self.parts.iter().zip(comps) {
            match part {
                Part::Literal(l) if l == comp => {}
                Part::Any => matched.push(*comp),
                Part::AnyLua if exact => matched.push(comp.strip_suffix(".lua")?),
                _ => return None,
            }
        }
        let key = if !matched.is_empty() {
            matched.join("/")
        } else if n == 0 {
            String::new()
        } else {
            self.path.clone()
        };
        let source_dir = comps[..n].join("/");
        let source_dir = if exact { source_dir.strip_suffix(".lua").map(str::to_string).unwrap_or(source_dir) } else { source_dir };
        Some((key, source_dir))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionSpec {
    pub title: String,
    pub groups: Vec<GroupSpec>,
}

/// The sections of the sidebar and the group paths in each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub sections: Vec<SectionSpec>,
}

/// Where a source file belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Index into `Layout::sections`.
    pub section: usize,
    /// Index of the matching group spec in that section's `groups`.
    pub group: usize,
    /// What the `*` components matched (joined with `/`), the path of a
    /// group without a `*`, or empty for `.`.
    pub key: String,
    /// Index in the section's `groups` of the first spec of the matching
    /// spec's family. A `*` path and the `*.lua` path that only differs in
    /// that last component form one family, so that a directory and a file
    /// of the same name are one group; every other spec is its own family.
    /// Section, family and key identify a group.
    pub family: usize,
    pub core: bool,
    /// The group's directory in the source tree, relative to the root. For
    /// a single-file group (`plugins/*.lua`) it is the file path without
    /// `.lua`, so that it coincides with a plugin folder of the same name.
    pub source_dir: String,
    /// Output directory relative to the docs root; it identifies the group.
    pub dir: String,
}

impl Placement {
    /// The last component of the key, used as the fallback group title.
    pub fn short_name(&self) -> &str {
        file_stem(&self.key)
    }
}

/// Group metadata from a `plugin.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
}

impl Layout {
    /// The built-in layout: the whole tree is the core, `packages/*` are
    /// packages and `plugins/*` and `plugins/*.lua` are plugins.
    pub fn default_for(title: &str) -> Layout {
        let spec = |path: &str, core: bool, dir: &str| {
            let mut g = GroupSpec::new(path, None, core).expect("valid built-in path");
            g.dir = dir.to_string();
            g
        };
        let mut sections = vec![
            SectionSpec { title: title.to_string(), groups: vec![spec(".", true, "core")] },
            SectionSpec { title: "Packages".to_string(), groups: vec![spec("packages/*", false, "packages")] },
            SectionSpec { title: "Plugins".to_string(), groups: vec![spec("plugins/*", false, "plugins"), spec("plugins/*.lua", false, "plugins")] },
        ];
        for s in &mut sections {
            assign_families(&mut s.groups);
        }
        Layout { sections }
    }

    /// A layout with output directories derived from the section titles:
    /// `<section>` for the first core group of a section,
    /// `<section>/<group>` for the other groups without a `*`, and
    /// `<section>/<key>` for the groups a `*` path makes. When a section has
    /// several `*` paths (other than a `*` and `*.lua` pair), each one's
    /// groups go to `<section>/<path>/<key>` instead, where `<path>` is the
    /// path's literal components joined with `-`. A directory that is taken
    /// already gets a `-2`, `-3`, ... suffix.
    pub fn new(mut sections: Vec<SectionSpec>) -> Layout {
        let mut used: HashSet<String> = HashSet::new();
        for s in &mut sections {
            let slug = unique_dir(&mut used, section_slug(&s.title));
            assign_families(&mut s.groups);
            let families = s.groups.iter().enumerate().filter(|(i, g)| g.is_pattern() && g.family == *i).count();
            let mut has_core = false;
            for i in 0..s.groups.len() {
                let g = &s.groups[i];
                let dir = if g.is_pattern() && g.family != i {
                    s.groups[g.family].dir.clone()
                } else if g.is_pattern() && families == 1 {
                    slug.clone()
                } else if g.is_pattern() {
                    unique_dir(&mut used, format!("{slug}/{}", g.literal_slug()))
                } else if g.core && !has_core {
                    has_core = true;
                    slug.clone()
                } else {
                    let name = g.name.as_deref().unwrap_or(if g.path == "." { "root" } else { file_stem(&g.path) });
                    unique_dir(&mut used, format!("{slug}/{}", slugify(name)))
                };
                s.groups[i].dir = dir;
            }
        }
        Layout { sections }
    }

    /// Finds the group a source path (relative, `/` separated) belongs to.
    /// The matching group path with the most literal components wins; on a
    /// tie, the one listed first.
    pub fn classify(&self, path: &str) -> Option<Placement> {
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let mut best: Option<(i32, Placement)> = None;
        for (si, section) in self.sections.iter().enumerate() {
            for (gi, g) in section.groups.iter().enumerate() {
                let Some((key, source_dir)) = g.matches(&comps) else { continue };
                let priority = g.priority();
                if best.as_ref().is_some_and(|(b, _)| *b >= priority) {
                    continue;
                }
                let dir = if g.is_pattern() { format!("{}/{}", g.dir, slugify(&key)) } else { g.dir.clone() };
                best = Some((priority, Placement { section: si, group: gi, key, family: g.family, core: g.core, source_dir, dir }));
            }
        }
        best.map(|(_, p)| p)
    }

    pub fn group(&self, p: &Placement) -> &GroupSpec {
        &self.sections[p.section].groups[p.group]
    }
}

/// Sets each group spec's family: the first spec with the same
/// `family_path`, or the spec itself.
fn assign_families(groups: &mut [GroupSpec]) {
    for i in 0..groups.len() {
        let family = groups[i].family_path();
        groups[i].family = groups[..i].iter().position(|g| family.is_some() && g.family_path() == family).unwrap_or(i);
    }
}

/// `base`, else `base-2`, `base-3`, ...: the first one not in `used`,
/// ignoring case. It is added to `used`.
fn unique_dir(used: &mut HashSet<String>, base: String) -> String {
    let mut dir = base.clone();
    let mut n = 1;
    while !used.insert(dir.to_ascii_lowercase()) {
        n += 1;
        dir = format!("{base}-{n}");
    }
    dir
}

fn section_slug(title: &str) -> String {
    let slug = slugify(&title.trim().to_ascii_lowercase().replace(' ', "-"));
    if slug.is_empty() { "section".to_string() } else { slug }
}

/// The contents of a config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub title: Option<String>,
    pub version: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub source_url: Option<String>,
    pub exclude: Vec<String>,
    pub documented_only: Option<bool>,
    /// The `sections`; `None` keeps the built-in layout.
    pub layout: Option<Layout>,
    /// Problems that do not stop the build, such as unknown keys.
    pub warnings: Vec<String>,
}

impl Config {
    pub fn from_yaml(text: &str) -> Result<Config, String> {
        let docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
        let mut config = Config::default();
        let root = match docs.into_iter().next() {
            None | Some(Yaml::Null) => return Ok(config),
            Some(Yaml::Hash(h)) => h,
            Some(_) => return Err("the top level must be a mapping".to_string()),
        };
        for (k, v) in &root {
            let key = key_name(k)?;
            match key.as_str() {
                "title" => config.title = scalar(v, &key)?,
                "version" => config.version = scalar(v, &key)?,
                "summary" => config.summary = scalar(v, &key)?,
                "description" => config.description = scalar(v, &key)?,
                "source_url" => config.source_url = scalar(v, &key)?,
                "exclude" => config.exclude = scalar_list(v, &key)?,
                "documented_only" => config.documented_only = boolean(v, &key)?,
                "sections" => config.layout = Some(Layout::new(sections(v, &mut config.warnings)?)),
                _ => config.warnings.push(format!("unknown key `{key}`")),
            }
        }
        Ok(config)
    }
}

fn key_name(k: &Yaml) -> Result<String, String> {
    match k {
        Yaml::String(s) | Yaml::Real(s) => Ok(s.clone()),
        Yaml::Integer(i) => Ok(i.to_string()),
        Yaml::Boolean(b) => Ok(b.to_string()),
        _ => Err("keys must be plain names".to_string()),
    }
}

fn scalar(v: &Yaml, what: &str) -> Result<Option<String>, String> {
    match v {
        Yaml::Null => Ok(None),
        Yaml::String(s) | Yaml::Real(s) => Ok(Some(s.clone())),
        Yaml::Integer(i) => Ok(Some(i.to_string())),
        Yaml::Boolean(b) => Ok(Some(b.to_string())),
        _ => Err(format!("`{what}` must be a string")),
    }
}

fn scalar_list(v: &Yaml, what: &str) -> Result<Vec<String>, String> {
    match v {
        Yaml::Array(items) => Ok(items.iter().map(|i| scalar(i, what)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect()),
        _ => Ok(scalar(v, what)?.into_iter().collect()),
    }
}

fn boolean(v: &Yaml, what: &str) -> Result<Option<bool>, String> {
    match v {
        Yaml::Null => Ok(None),
        Yaml::Boolean(b) => Ok(Some(*b)),
        _ => Err(format!("`{what}` must be true or false")),
    }
}

fn mapping<'a>(v: &'a Yaml, what: &str) -> Result<&'a Hash, String> {
    match v {
        Yaml::Hash(h) => Ok(h),
        _ => Err(format!("{what} must be a mapping")),
    }
}

fn list<'a>(v: &'a Yaml, what: &str) -> Result<&'a [Yaml], String> {
    match v {
        Yaml::Array(items) => Ok(items),
        Yaml::Null => Ok(&[]),
        _ => Err(format!("{what} must be a list")),
    }
}

fn sections(v: &Yaml, warnings: &mut Vec<String>) -> Result<Vec<SectionSpec>, String> {
    let mut out = Vec::new();
    for (i, s) in list(v, "`sections`")?.iter().enumerate() {
        let at = format!("sections[{i}]");
        let mut title = None;
        let mut groups = Vec::new();
        for (k, v) in mapping(s, &format!("`{at}`"))? {
            let key = key_name(k)?;
            match key.as_str() {
                "title" => title = scalar(v, &format!("{at}.title"))?,
                "groups" => {
                    for (j, g) in list(v, &format!("`{at}.groups`"))?.iter().enumerate() {
                        groups.push(group(g, &format!("{at}.groups[{j}]"), warnings)?);
                    }
                }
                _ => warnings.push(format!("unknown key `{key}` in {at}")),
            }
        }
        let title = title.ok_or_else(|| format!("`{at}` has no `title`"))?;
        out.push(SectionSpec { title, groups });
    }
    Ok(out)
}

fn group(v: &Yaml, at: &str, warnings: &mut Vec<String>) -> Result<GroupSpec, String> {
    let mut path = None;
    let mut name = None;
    let mut core = false;
    for (k, v) in mapping(v, &format!("`{at}`"))? {
        let key = key_name(k)?;
        match key.as_str() {
            "path" => path = scalar(v, &format!("{at}.path"))?,
            "name" => name = scalar(v, &format!("{at}.name"))?,
            "core" => core = boolean(v, &format!("{at}.core"))?.unwrap_or(false),
            _ => warnings.push(format!("unknown key `{key}` in {at}")),
        }
    }
    let path = path.ok_or_else(|| format!("`{at}` has no `path`"))?;
    GroupSpec::new(&path, name, core)
}

/// Reads a Flux-style `plugin.yml`: a mapping with `name`, `description`,
/// `author` and `version`. Other keys are ignored.
pub fn parse_plugin_yml(text: &str) -> Result<GroupMeta, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
    let mut meta = GroupMeta::default();
    let root = match docs.into_iter().next() {
        None | Some(Yaml::Null) => return Ok(meta),
        Some(Yaml::Hash(h)) => h,
        Some(_) => return Err("the top level must be a mapping".to_string()),
    };
    for (k, v) in &root {
        let key = key_name(k)?;
        let slot = match key.as_str() {
            "name" => &mut meta.name,
            "description" => &mut meta.description,
            "author" => &mut meta.author,
            "version" => &mut meta.version,
            _ => continue,
        };
        if let Some(value) = scalar(v, &key)?.filter(|v| !v.trim().is_empty()) {
            slot.get_or_insert(value.trim().to_string());
        }
    }
    Ok(meta)
}

/// Reads the `[Plugin]` section of a Clockwork `plugin.ini`:
/// `name="Stamina"`, `author`, `description` and `compatibility` (or
/// `version`). Values may be quoted and lines may end with `;`.
pub fn parse_plugin_ini(text: &str) -> GroupMeta {
    let mut meta = GroupMeta::default();
    let mut compatibility = None;
    let mut in_plugin = true;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_plugin = section.trim().eq_ignore_ascii_case("plugin");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if !in_plugin || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = value.trim().trim_end_matches(';').trim_end();
        let value = match value.strip_prefix('"') {
            Some(v) => v.split('"').next().unwrap_or(v),
            None => value,
        };
        if value.is_empty() {
            continue;
        }
        let slot = match key.as_str() {
            "name" => &mut meta.name,
            "description" => &mut meta.description,
            "author" => &mut meta.author,
            "version" => &mut meta.version,
            "compatibility" => &mut compatibility,
            _ => continue,
        };
        slot.get_or_insert_with(|| value.to_string());
    }
    if meta.version.is_none() {
        meta.version = compatibility;
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(layout: &Layout, path: &str) -> Option<(usize, String, String, bool)> {
        layout.classify(path).map(|p| (p.section, p.key, p.dir, p.core))
    }

    #[test]
    fn default_layout_matches_flux() {
        let l = Layout::default_for("Flux");
        assert_eq!(l.sections[0].title, "Flux");
        assert_eq!(place(&l, "gamemode/core/sh_util.lua"), Some((0, String::new(), "core".into(), true)));
        assert_eq!(place(&l, "packagespec.lua"), Some((0, String::new(), "core".into(), true)));
        assert_eq!(place(&l, "packages/flow/views/a.lua"), Some((1, "flow".into(), "packages/flow".into(), false)));
        assert_eq!(place(&l, "packages/loose.lua").map(|p| p.0), Some(0), "files directly in packages/ are core");
        assert_eq!(place(&l, "plugins/characters/plugin/sh_plugin.lua"), Some((2, "characters".into(), "plugins/characters".into(), false)));
        let file = l.classify("plugins/sh_crosshair.lua").unwrap();
        assert_eq!((file.key.as_str(), file.dir.as_str(), file.source_dir.as_str()), ("sh_crosshair", "plugins/sh_crosshair", "plugins/sh_crosshair"));
        assert_eq!(l.classify("plugins/areas/sh_areas.lua").unwrap().source_dir, "plugins/areas");
    }

    const CATWORK: &str = "
title: Catwork
version: \"0.95\"
exclude: [thirdparty, gamemodes/catwork/gamemode/thirdparty]
documented_only: false
colour: red
sections:
  - title: Catwork
    groups:
      - path: gamemodes/catwork/gamemode
        name: Catwork
        core: true
  - title: Plugins
    groups:
      - path: gamemodes/catwork/plugins/*
      - path: gamemodes/catwork/plugins/*.lua
  - title: HL2RP
    icon: x
    groups:
      - path: ./gamemodes/cwhl2rp/schema/
        name: HL2RP
        core: true
      - path: gamemodes/cwhl2rp/plugins/*
";

    #[test]
    fn reads_config() {
        let c = Config::from_yaml(CATWORK).unwrap();
        assert_eq!(c.title.as_deref(), Some("Catwork"));
        assert_eq!(c.version.as_deref(), Some("0.95"));
        assert_eq!(c.exclude, vec!["thirdparty", "gamemodes/catwork/gamemode/thirdparty"]);
        assert_eq!(c.documented_only, Some(false));
        assert_eq!(c.warnings, vec!["unknown key `colour`", "unknown key `icon` in sections[2]"]);
        let l = c.layout.unwrap();
        assert_eq!(l.sections.len(), 3);
        assert_eq!(l.sections[2].groups[0].path, "gamemodes/cwhl2rp/schema");

        let core = l.classify("gamemodes/catwork/gamemode/core/sh_kernel.lua").unwrap();
        assert_eq!((core.section, core.core, core.dir.as_str()), (0, true, "catwork"));
        assert_eq!(l.group(&core).name.as_deref(), Some("Catwork"));
        let p = l.classify("gamemodes/catwork/plugins/stamina/plugin/sh_plugin.lua").unwrap();
        assert_eq!((p.section, p.key.as_str(), p.dir.as_str(), p.source_dir.as_str()), (1, "stamina", "plugins/stamina", "gamemodes/catwork/plugins/stamina"));
        let f = l.classify("gamemodes/catwork/plugins/sh_raisegun.lua").unwrap();
        assert_eq!((f.section, f.key.as_str(), f.dir.as_str()), (1, "sh_raisegun", "plugins/sh_raisegun"));
        assert_eq!(place(&l, "gamemodes/cwhl2rp/schema/sh_schema.lua"), Some((2, "gamemodes/cwhl2rp/schema".into(), "hl2rp".into(), true)));
        assert_eq!(place(&l, "gamemodes/cwhl2rp/plugins/voices/plugin/sh_plugin.lua"), Some((2, "voices".into(), "hl2rp/voices".into(), false)));
        assert_eq!(l.classify("gamemodes/cwhl2rp/gamemode/init.lua"), None);
        assert_eq!(l.classify("gamemodes/catwork/plugins"), None);
    }

    #[test]
    fn most_literal_path_wins() {
        let c = Config::from_yaml(
            "sections:
  - title: All
    groups:
      - path: .
        core: true
      - path: '*'
  - title: Plugins
    groups:
      - path: lib/plugins/*
      - path: lib/*/x
",
        )
        .unwrap();
        let l = c.layout.unwrap();
        assert_eq!(place(&l, "init.lua"), Some((0, String::new(), "all".into(), true)));
        assert_eq!(place(&l, "lib/a.lua"), Some((0, "lib".into(), "all/lib".into(), false)));
        assert_eq!(place(&l, "lib/plugins/a/b.lua"), Some((1, "a".into(), "plugins/lib-plugins/a".into(), false)));
        assert_eq!(place(&l, "lib/plugins/x/b.lua").map(|p| p.1), Some("x".into()), "ties go to the first path");
        assert_eq!(place(&l, "lib/y/x/b.lua"), Some((1, "y".into(), "plugins/lib-x/y".into(), false)));
    }

    #[test]
    fn output_directories_do_not_collide() {
        let c = Config::from_yaml(
            "sections:
  - title: Core Stuff
    groups:
      - { path: a, core: true }
      - { path: b, core: true }
      - { path: c/d.lua }
  - title: core stuff
    groups:
      - { path: e, core: true }
",
        )
        .unwrap();
        let l = c.layout.unwrap();
        let dirs: Vec<String> = ["a/1.lua", "b/1.lua", "c/d.lua", "e/1.lua"].iter().map(|p| l.classify(p).unwrap().dir).collect();
        assert_eq!(dirs, vec!["core-stuff", "core-stuff/b", "core-stuff/d", "core-stuff-2"]);
    }

    #[test]
    fn gives_every_group_spec_its_own_directory() {
        let c = Config::from_yaml(
            "sections:
  - title: Plugins
    groups:
      - path: gamemodes/a/plugins/*
      - path: gamemodes/a/plugins/*.lua
      - path: gamemodes/b/plugins/*
      - { path: extra/x, name: Stamina }
      - { path: more/x, name: stamina }
  - title: Other
    groups:
      - path: lib/*
      - { path: misc/stamina, name: stamina }
",
        )
        .unwrap();
        let l = c.layout.unwrap();
        let dirs: Vec<String> = ["gamemodes/a/plugins/stamina/sh_plugin.lua", "gamemodes/a/plugins/stamina.lua", "gamemodes/b/plugins/stamina/sh_plugin.lua", "extra/x/a.lua", "more/x/a.lua", "lib/stamina/a.lua", "misc/stamina/a.lua"]
            .iter()
            .map(|p| l.classify(p).unwrap().dir)
            .collect();
        assert_eq!(
            dirs,
            vec!["plugins/gamemodes-a-plugins/stamina", "plugins/gamemodes-a-plugins/stamina", "plugins/gamemodes-b-plugins/stamina", "plugins/Stamina", "plugins/stamina-2", "other/stamina", "other/stamina"]
        );
        let families: Vec<usize> = ["gamemodes/a/plugins/s/a.lua", "gamemodes/a/plugins/s.lua", "gamemodes/b/plugins/s/a.lua", "extra/x/a.lua"].iter().map(|p| l.classify(p).unwrap().family).collect();
        assert_eq!(families, vec![0, 0, 2, 3], "a `*` path and its `*.lua` path form one family");
    }

    #[test]
    fn rejects_bad_config() {
        assert!(Config::from_yaml("sections:\n  - title: [a\n").unwrap_err().contains("line"));
        assert!(Config::from_yaml("- a\n- b\n").is_err());
        assert!(Config::from_yaml("sections:\n  - groups: []\n").unwrap_err().contains("title"));
        assert!(Config::from_yaml("sections:\n  - title: A\n    groups:\n      - path: a/**\n").is_err());
        assert!(Config::from_yaml("sections:\n  - title: A\n    groups:\n      - path: '*.lua/a'\n").is_err());
        assert!(Config::from_yaml("documented_only: maybe\n").is_err());
        assert_eq!(Config::from_yaml("").unwrap(), Config::default());
        assert_eq!(Config::from_yaml("title: X\n").unwrap().layout, None);
    }

    #[test]
    fn reads_plugin_yml() {
        let meta = parse_plugin_yml("name: 3D Texts\ndescription: Adds 3D texts.\nauthor: TeslaCloud Studios\nversion: 1.2\ndepends:\n  - admin\n").unwrap();
        assert_eq!(meta, GroupMeta { name: Some("3D Texts".into()), description: Some("Adds 3D texts.".into()), author: Some("TeslaCloud Studios".into()), version: Some("1.2".into()) });
        assert_eq!(parse_plugin_yml("").unwrap(), GroupMeta::default());
        assert!(parse_plugin_yml("- a\n- b").is_err());
    }

    #[test]
    fn reads_plugin_ini() {
        let meta = parse_plugin_ini("[Plugin]\nname=\"Extra Voices\"\nauthor=\"kurozael\"\ndescription=\"Adds voices; and more.\"\ncompatibility=\"0.93\";\n\n--[[\n\twritten by x = y\n--]]");
        assert_eq!(meta, GroupMeta { name: Some("Extra Voices".into()), description: Some("Adds voices; and more.".into()), author: Some("kurozael".into()), version: Some("0.93".into()) });
        let meta = parse_plugin_ini("[Other]\nname=\"No\"\n[plugin]\nname = Yes ;\nversion=\"2\"\ncompatibility=\"1\"");
        assert_eq!((meta.name.as_deref(), meta.version.as_deref()), (Some("Yes"), Some("2")));
    }
}
