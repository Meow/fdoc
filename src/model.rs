//! Builds the documentation model (sections, groups, modules, functions)
//! from the scanned files, and the cross-reference index used for links.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::doc::DocBlock;
use crate::layout::{GroupMeta, Layout, Placement};
use crate::scanner::{Category, FileScan, Sep};

/// Template objects that are redefined per file (`PANEL:Init`, ...), so
/// each file becomes its own module.
const PER_FILE_OBJECTS: [&str; 14] = ["PANEL", "CMD", "SKIN", "THEME", "TOOL", "ENT", "SWEP", "EFFECT", "ROLE", "PACKAGE", "ITEM", "ATTRIBUTE", "FACTION", "CONDITION"];

/// Conventional names for metatable locals when the file does not declare them.
const META_NAMES: [(&str, &str); 12] = [
    ("player_meta", "Player"),
    ("ent_meta", "Entity"),
    ("entity_meta", "Entity"),
    ("panel_meta", "Panel"),
    ("color_meta", "Color"),
    ("number_meta", "Number"),
    ("string_meta", "String"),
    ("vector_meta", "Vector"),
    ("angle_meta", "Angle"),
    ("weapon_meta", "Weapon"),
    ("npc_meta", "NPC"),
    ("vehicle_meta", "Vehicle"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleKind {
    Class { extends: Option<String> },
    Library,
    Hooks,
    Globals,
    /// A per-file template object such as a `PANEL`.
    Object { object: String },
}

impl ModuleKind {
    pub fn badge(&self) -> &str {
        match self {
            ModuleKind::Class { .. } => "class",
            ModuleKind::Library => "library",
            ModuleKind::Hooks => "hooks",
            ModuleKind::Globals => "globals",
            ModuleKind::Object { .. } => "object",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    /// Display owner, e.g. `ActiveRecord.Base`; empty for globals.
    pub owner: String,
    pub sep: Sep,
    pub params: Vec<String>,
    pub doc: Option<DocBlock>,
    pub file: String,
    pub line: usize,
    pub anchor: String,
    pub category: String,
}

impl Function {
    pub fn qualified(&self) -> String {
        match self.sep {
            Sep::None => self.name.clone(),
            Sep::Dot => format!("{}.{}", self.owner, self.name),
            Sep::Colon => format!("{}:{}", self.owner, self.name),
        }
    }

    pub fn signature(&self) -> String {
        format!("{}({})", self.name, self.params.join(", "))
    }

    pub fn qualified_signature(&self) -> String {
        format!("{}({})", self.qualified(), self.params.join(", "))
    }

    pub fn summary(&self) -> String {
        self.doc.as_ref().map(|d| d.summary()).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Module {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub slug: String,
    pub kind: ModuleKind,
    pub doc: Option<DocBlock>,
    pub categories: Vec<Category>,
    pub functions: Vec<Function>,
    pub files: Vec<String>,
    pub decl_file: Option<(String, usize)>,
}

impl Module {
    /// Functions grouped by category, in category order.
    pub fn grouped(&self) -> Vec<(&Category, Vec<&Function>)> {
        self.categories.iter().map(|c| (c, self.functions.iter().filter(|f| f.category == c.name).collect::<Vec<_>>())).filter(|(_, fs)| !fs.is_empty()).collect()
    }

    pub fn summary(&self) -> String {
        self.doc.as_ref().map(|d| d.summary()).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub title: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    /// Output directory relative to the docs root, e.g. `packages/flow`.
    pub dir: String,
    pub modules: Vec<Module>,
    /// Core groups have no index page of their own.
    pub is_core: bool,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub title: String,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub title: String,
    pub version: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub sections: Vec<Section>,
    /// Reference key -> URL relative to the docs root.
    pub index: HashMap<String, String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOptions<'a> {
    /// Explicit project name; overrides the packagespec.
    pub title: Option<&'a str>,
    /// Name used when neither `title` nor a packagespec provides one.
    pub fallback_title: Option<&'a str>,
    /// Explicit project version, summary and description; override the
    /// packagespec.
    pub version: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub description: Option<&'a str>,
    pub documented_only: bool,
    pub source_url: Option<&'a str>,
    /// Which files form which groups and sections.
    pub layout: &'a Layout,
    /// Metadata from `plugin.ini` files, by the group's output directory
    /// (`Placement::dir`).
    pub group_meta: &'a HashMap<String, GroupMeta>,
}

impl<'a> BuildOptions<'a> {
    /// Options with nothing set apart from the layout and group metadata.
    pub fn new(layout: &'a Layout, group_meta: &'a HashMap<String, GroupMeta>) -> BuildOptions<'a> {
        BuildOptions { title: None, fallback_title: None, version: None, summary: None, description: None, documented_only: false, source_url: None, layout, group_meta }
    }
}

fn file_stem(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).trim_end_matches(".lua").to_string()
}

pub fn slugify(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect()
}

struct GroupBuilder {
    key: String,
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    version: Option<String>,
    plugin_global: Option<String>,
    files: Vec<(String, FileScan)>,
}

/// The first value of `key` in the root `packagespec.lua`.
fn root_spec(files: &[(String, FileScan)], key: &str) -> Option<String> {
    files.iter().filter(|(path, _)| path == "packagespec.lua").flat_map(|(_, scan)| &scan.spec).find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// The project name: `title`, else the root packagespec's name, else
/// `fallback`.
pub fn project_title(files: &[(String, FileScan)], title: Option<&str>, fallback: Option<&str>) -> String {
    title.map(str::to_string).or_else(|| root_spec(files, "name")).or_else(|| fallback.map(str::to_string)).unwrap_or_else(|| "Documentation".to_string())
}

/// Metadata of a group's own `packagespec.lua`.
fn package_meta(scan: &FileScan) -> GroupMeta {
    let mut meta = GroupMeta::default();
    for (k, v) in &scan.spec {
        let slot = match k.as_str() {
            "name" => &mut meta.name,
            "summary" | "description" => &mut meta.description,
            "author" => &mut meta.author,
            "version" => &mut meta.version,
            _ => continue,
        };
        slot.get_or_insert_with(|| v.clone());
    }
    meta
}

pub fn build(files: Vec<(String, FileScan)>, opts: BuildOptions) -> Project {
    let title = project_title(&files, opts.title, opts.fallback_title);
    let version = opts.version.map(str::to_string).or_else(|| root_spec(&files, "version"));
    let summary = opts.summary.map(str::to_string).or_else(|| root_spec(&files, "summary"));
    let description = opts.description.map(str::to_string).or_else(|| root_spec(&files, "description"));
    let layout = opts.layout;

    // Gather files per group, keyed by output directory. `PLUGIN:set_*`
    // calls fill the builder; the group's own packagespec is kept aside.
    let mut groups: BTreeMap<String, (Placement, GroupMeta, GroupBuilder)> = BTreeMap::new();
    for (path, scan) in files {
        let Some(place) = layout.classify(&path) else { continue };
        let (place, spec, g) = groups.entry(place.dir.clone()).or_insert_with(|| {
            let g = GroupBuilder { key: place.key.clone(), name: None, description: None, author: None, version: None, plugin_global: None, files: Vec::new() };
            (place, GroupMeta::default(), g)
        });
        if let Some(n) = &scan.plugin_name {
            g.name.get_or_insert(n.clone());
        }
        if let Some(d) = &scan.plugin_description {
            g.description.get_or_insert(d.clone());
        }
        if let Some(a) = &scan.plugin_author {
            g.author.get_or_insert(a.clone());
        }
        if let Some(gl) = &scan.plugin_global {
            g.plugin_global.get_or_insert(gl.clone());
        }
        if !place.core && path == format!("{}/packagespec.lua", place.source_dir) {
            *spec = package_meta(&scan);
        }
        g.files.push((path, scan));
    }

    let mut sections: Vec<Section> = layout.sections.iter().map(|s| Section { title: s.title.clone(), groups: Vec::new() }).collect();
    let mut placed: Vec<(Placement, Group)> = Vec::new();
    let mut dirs: HashSet<String> = HashSet::new();
    for (place, spec, mut gb) in groups.into_values() {
        // Name priority: the layout, plugin.ini, packagespec, `PLUGIN:set_name`,
        // the path. Core groups are named after their section instead.
        let fixed = layout.group(&place).name.clone();
        let ini = opts.group_meta.get(&place.dir).cloned().unwrap_or_default();
        let section_title = &layout.sections[place.section].title;
        let group_title = if place.core {
            let title = fixed.clone().or(ini.name.clone()).unwrap_or_else(|| section_title.clone());
            gb.name = fixed.clone().or(ini.name.clone()).or(gb.name);
            title
        } else {
            gb.name = fixed.or(ini.name).or(spec.name).or(gb.name);
            gb.name.clone().unwrap_or_else(|| if place.key.is_empty() { section_title.clone() } else { place.short_name().to_string() })
        };
        gb.description = ini.description.or(spec.description).or(gb.description);
        gb.author = ini.author.or(spec.author).or(gb.author);
        gb.version = ini.version.or(spec.version).or(gb.version);

        let modules = build_modules(&gb, &group_title, opts.documented_only);
        // Output directories must also differ on case-insensitive file systems.
        let mut dir = place.dir.clone();
        let mut n = 1;
        while !dirs.insert(dir.to_ascii_lowercase()) {
            n += 1;
            dir = format!("{}-{n}", place.dir);
        }
        let group = Group { title: group_title, description: gb.description, author: gb.author, version: gb.version, dir, modules, is_core: place.core };
        placed.push((place, group));
    }
    // Core groups first, then by key.
    placed.sort_by(|(a, _), (b, _)| b.core.cmp(&a.core).then_with(|| a.key.to_ascii_lowercase().cmp(&b.key.to_ascii_lowercase())).then_with(|| a.dir.cmp(&b.dir)));
    for (place, group) in placed {
        sections[place.section].groups.push(group);
    }
    sections.retain(|s| !s.groups.is_empty());

    let mut project = Project { title, version, summary, description, sections, index: HashMap::new(), source_url: opts.source_url.map(str::to_string) };
    project.index = build_index(&project);
    project
}

fn resolve_owner(owner: &str, scan: &FileScan, plugin_global: &Option<String>) -> String {
    let mut owner = owner.replace("::", ".");
    if let Some((_, target)) = scan.meta_aliases.iter().find(|(a, _)| *a == owner) {
        owner = target.clone();
    } else if let Some((_, target)) = META_NAMES.iter().find(|(a, _)| *a == owner) {
        owner = target.to_string();
    }
    if let (true, Some(g)) = (owner == "PLUGIN", plugin_global) {
        owner = g.clone();
    }
    owner
}

fn build_modules(gb: &GroupBuilder, group_title: &str, documented_only: bool) -> Vec<Module> {
    // Module key -> module. Per-file objects are keyed by (object, file).
    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    let plugin_global = &gb.plugin_global;

    // Class declarations first, so modules get their docs and kind.
    for (path, scan) in &gb.files {
        for class in &scan.classes {
            let id = class.name.replace("::", ".");
            let m = modules.entry(id.to_ascii_lowercase()).or_insert_with(|| new_module(&id, ModuleKind::Class { extends: None }));
            m.kind = ModuleKind::Class { extends: class.extends.as_ref().map(|e| e.replace("::", ".")) };
            if m.doc.is_none() {
                m.doc = class.doc.clone();
            }
            if m.decl_file.is_none() {
                m.decl_file = Some((path.clone(), class.line));
            }
            if !m.files.contains(path) {
                m.files.push(path.clone());
            }
        }
    }

    for (path, scan) in &gb.files {
        // Categories declared in this file, by name.
        let file_categories: HashMap<&str, &Category> = scan.categories.iter().map(|c| (c.name.as_str(), c)).collect();

        for f in &scan.functions {
            if documented_only && f.doc.is_none() {
                continue;
            }
            let raw_owner = f.owner.clone();
            let owner = resolve_owner(&raw_owner, scan, plugin_global);
            let first = owner.split('.').next().unwrap_or("").to_string();

            let (key, id, title, subtitle, kind) = if owner.is_empty() {
                ("globals".to_string(), "Globals".to_string(), "Globals".to_string(), Some(format!("Global functions of {group_title}")), ModuleKind::Globals)
            } else if PER_FILE_OBJECTS.contains(&first.as_str()) {
                let name = if first == "PANEL" { scan.vgui_name.clone().unwrap_or_else(|| file_stem(path)) } else { file_stem(path) };
                (format!("{}\u{0}{}", first, path), name.clone(), name, Some(first.clone()), ModuleKind::Object { object: first.clone() })
            } else if owner == "GM" {
                (owner.to_ascii_lowercase(), owner.clone(), owner.clone(), Some("Gamemode hooks".to_string()), ModuleKind::Hooks)
            } else if raw_owner == "PLUGIN" || plugin_global.as_deref() == Some(owner.as_str()) {
                let id = if owner == "PLUGIN" { gb.name.clone().unwrap_or_else(|| gb.key.clone()) } else { owner.clone() };
                (owner.to_ascii_lowercase(), id.clone(), id, Some("Plugin hooks and methods".to_string()), ModuleKind::Hooks)
            } else {
                (owner.to_ascii_lowercase(), owner.clone(), owner.clone(), None, ModuleKind::Library)
            };

            let m = modules.entry(key).or_insert_with(|| {
                let mut m = new_module(&id, kind.clone());
                m.title = title;
                m.subtitle = subtitle;
                m
            });
            if !m.files.contains(path) {
                m.files.push(path.clone());
            }
            if matches!(m.kind, ModuleKind::Library) && f.sep == Sep::Colon {
                m.kind = ModuleKind::Class { extends: None };
            }

            let category = f.category.clone().unwrap_or_else(|| match (&m.kind, f.sep) {
                (ModuleKind::Hooks, _) if owner == "GM" => "Hooks".to_string(),
                (_, Sep::Colon) => "Methods".to_string(),
                _ => "Functions".to_string(),
            });
            if !m.categories.iter().any(|c| c.name == category) {
                let description = file_categories.get(category.as_str()).map(|c| c.description.clone()).unwrap_or_default();
                m.categories.push(Category { name: category.clone(), description });
            }

            m.functions.push(Function { name: f.name.clone(), owner: owner.clone(), sep: f.sep, params: f.params.clone(), doc: f.doc.clone(), file: path.clone(), line: f.line, anchor: String::new(), category });
        }
    }

    // Keep declared classes that have no functions only when documented.
    modules.retain(|_, m| !m.functions.is_empty() || m.doc.is_some());

    let mut out: Vec<Module> = modules.into_values().collect();

    // Per-file objects may share a title inside a group; make ids unique.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for m in &mut out {
        let n = seen.entry(m.id.to_ascii_lowercase()).or_insert(0);
        *n += 1;
        if *n > 1 {
            let suffix = match &m.kind {
                ModuleKind::Object { object } => object.clone(),
                _ => n.to_string(),
            };
            m.id = format!("{}.{}", m.id, suffix);
            m.title = m.id.clone();
        }
        m.slug = slugify(&m.id);
    }
    // Slugs must be unique on case-insensitive file systems.
    let mut slugs: HashSet<String> = HashSet::new();
    for m in &mut out {
        let mut slug = m.slug.clone();
        let mut n = 1;
        while !slugs.insert(slug.to_ascii_lowercase()) {
            n += 1;
            slug = format!("{}-{n}", m.slug);
        }
        m.slug = slug;
    }

    for m in &mut out {
        // Default categories first, then custom ones in order of appearance.
        m.categories.sort_by_key(|c| match c.name.as_str() {
            "Hooks" => 0,
            "Functions" => 1,
            "Methods" => 2,
            _ => 3,
        });
        m.functions.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()).then(a.file.cmp(&b.file)));
        let mut anchors: HashSet<String> = HashSet::new();
        for f in &mut m.functions {
            let base = slugify(&f.name);
            let mut anchor = base.clone();
            let mut n = 1;
            while !anchors.insert(anchor.clone()) {
                n += 1;
                anchor = format!("{base}-{n}");
            }
            f.anchor = anchor;
        }
    }

    out.sort_by(|a, b| {
        let rank = |m: &Module| match m.kind {
            ModuleKind::Hooks => 0,
            ModuleKind::Globals => 1,
            _ => 2,
        };
        rank(a).cmp(&rank(b)).then(a.id.to_ascii_lowercase().cmp(&b.id.to_ascii_lowercase()))
    });
    out
}

fn new_module(id: &str, kind: ModuleKind) -> Module {
    Module { id: id.to_string(), title: id.to_string(), subtitle: None, slug: slugify(id), kind, doc: None, categories: Vec::new(), functions: Vec::new(), files: Vec::new(), decl_file: None }
}

fn build_index(project: &Project) -> HashMap<String, String> {
    let mut index = HashMap::new();
    let mut insert = |key: String, url: &str| {
        index.entry(key).or_insert_with(|| url.to_string());
    };
    for section in &project.sections {
        for group in &section.groups {
            for m in &group.modules {
                let page = format!("{}/{}.html", group.dir, m.slug);
                insert(m.id.clone(), &page);
                insert(m.id.replace('.', "::"), &page);
                if let ModuleKind::Object { object } = &m.kind {
                    insert(format!("{object}:{}", m.id), &page);
                }
                for f in &m.functions {
                    let url = format!("{page}#{}", f.anchor);
                    if f.owner.is_empty() {
                        insert(f.name.clone(), &url);
                        continue;
                    }
                    let owners = [f.owner.clone(), f.owner.replace('.', "::"), m.id.clone()];
                    for owner in owners.iter().collect::<HashSet<_>>() {
                        for sep in ['#', ':', '.'] {
                            insert(format!("{owner}{sep}{}", f.name), &url);
                        }
                    }
                    // `player_meta#name` style references use the local name.
                    for (alias, target) in META_NAMES {
                        if target == f.owner {
                            for sep in ['#', ':', '.'] {
                                insert(format!("{alias}{sep}{}", f.name), &url);
                            }
                        }
                    }
                    if let Some(doc) = &f.doc {
                        for alias in &doc.aliases {
                            insert(alias.replace("::", "."), &url);
                        }
                    }
                }
            }
        }
    }
    index
}

impl Project {
    /// Resolves a reference such as `Owner#name`, `Owner.name`, `name` or
    /// `Owner` to a URL relative to the docs root.
    pub fn resolve(&self, reference: &str) -> Option<String> {
        let r = reference.trim().trim_end_matches("()");
        if r.is_empty() || r.contains(char::is_whitespace) {
            return None;
        }
        if let Some(url) = self.index.get(r) {
            return Some(url.clone());
        }
        let normalized = r.replace("::", ".");
        if let Some(url) = self.index.get(&normalized) {
            return Some(url.clone());
        }
        for sep in ['#', ':', '.'] {
            if let Some(p) = normalized.rfind(sep) {
                let key = format!("{}#{}", &normalized[..p], &normalized[p + 1..]);
                if let Some(url) = self.index.get(&key) {
                    return Some(url.clone());
                }
            }
        }
        None
    }

    pub fn all_modules(&self) -> impl Iterator<Item = (&Group, &Module)> {
        self.sections.iter().flat_map(|s| s.groups.iter().flat_map(|g| g.modules.iter().map(move |m| (g, m))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::scan;

    fn files() -> Vec<(String, FileScan)> {
        vec![
            ("packagespec.lua".to_string(), scan("Package:describe(function(s)\n  s.name = 'Flux'\n  s.version = '1.0'\nend)")),
            ("lib/a.lua".to_string(), scan("--- Class A.\nclass 'Core::A'\n--- Does x.\n-- @see [Core::A#other]\nfunction Core.A:do_x(a)\nend\nfunction Core.A:other() end\n--- Global.\nfunction g() end\nlocal player_meta = FindMetaTable('Player')\nfunction player_meta:jump() end")),
            ("plugins/sh_crosshair.lua".to_string(), scan("PLUGIN:set_name('Crosshair')\n--- Paints.\nfunction PLUGIN:HUDPaint() end")),
            ("plugins/characters/plugin/sh_plugin.lua".to_string(), scan("PLUGIN:set_global('Characters')\nfunction Characters:OnLoaded() end\nfunction PLUGIN:Init() end")),
            ("packages/flow/views/a.lua".to_string(), scan("function PANEL:Init() end\nvgui.Register('fl_a', PANEL)")),
            ("packages/flow/views/b.lua".to_string(), scan("function PANEL:Init() end\nvgui.Register('fl_b', PANEL)")),
            ("packages/flow/packagespec.lua".to_string(), scan("s.name = 'Flow'\ns.summary = 'Core.'")),
        ]
    }

    #[test]
    fn builds_sections_and_modules() {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let p = build(files(), BuildOptions { fallback_title: Some("fallback"), ..BuildOptions::new(&layout, &meta) });
        assert_eq!(p.title, "Flux");
        assert_eq!(p.version.as_deref(), Some("1.0"));
        assert_eq!(p.sections.len(), 3);
        let core = &p.sections[0].groups[0];
        let ids: Vec<&str> = core.modules.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["Globals", "Core.A", "Player"]);
        let a = &core.modules[1];
        assert!(matches!(a.kind, ModuleKind::Class { .. }));
        assert_eq!(a.summary(), "Class A.");
        assert_eq!(a.functions.len(), 2);
        assert_eq!(a.categories[0].name, "Methods");

        let packages = &p.sections[1].groups[0];
        assert_eq!(packages.title, "Flow");
        assert_eq!(packages.description.as_deref(), Some("Core."));
        let names: Vec<&str> = packages.modules.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(names, vec!["fl_a", "fl_b"]);

        let plugins = &p.sections[2];
        assert_eq!(plugins.groups[0].title, "Characters".to_string().replace("Characters", "characters"));
        let chars = &plugins.groups[0].modules[0];
        assert_eq!(chars.id, "Characters");
        assert_eq!(chars.functions.len(), 2, "PLUGIN and the global merge");
        let crosshair = &plugins.groups[1];
        assert_eq!(crosshair.title, "Crosshair");
        assert_eq!(crosshair.modules[0].id, "Crosshair");
        assert_eq!(crosshair.dir, "plugins/sh_crosshair");
    }

    #[test]
    fn groups_by_configured_layout() {
        let config = crate::layout::Config::from_yaml(
            "sections:
  - title: Catwork
    groups:
      - { path: gm/gamemode, core: true }
  - title: Plugins
    groups:
      - path: gm/plugins/*
      - path: gm/plugins/*.lua
",
        )
        .unwrap();
        let layout = config.layout.unwrap();
        let mut meta = HashMap::new();
        meta.insert("plugins/stamina".to_string(), GroupMeta { name: Some("Stamina".into()), description: None, author: Some("kurozael".into()), version: Some("0.93".into()) });
        let files = vec![
            ("gm/gamemode/core/sh_kernel.lua".to_string(), scan("function Kernel.Init() end")),
            ("gm/plugins/stamina/plugin/sh_plugin.lua".to_string(), scan("PLUGIN:set_name('Ignored')\nPLUGIN:set_description('From code.')\nfunction PLUGIN:Think() end")),
            ("gm/plugins/sh_raisegun.lua".to_string(), scan("function PLUGIN:Think() end")),
            ("gm/other/x.lua".to_string(), scan("function x() end")),
        ];
        let p = build(files, BuildOptions { title: Some("Catwork"), ..BuildOptions::new(&layout, &meta) });
        let sections: Vec<&str> = p.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(sections, vec!["Catwork", "Plugins"]);
        let core = &p.sections[0].groups[0];
        assert_eq!((core.title.as_str(), core.dir.as_str(), core.is_core), ("Catwork", "catwork", true));
        let plugins: Vec<(&str, &str)> = p.sections[1].groups.iter().map(|g| (g.title.as_str(), g.dir.as_str())).collect();
        assert_eq!(plugins, vec![("sh_raisegun", "plugins/sh_raisegun"), ("Stamina", "plugins/stamina")]);
        let stamina = &p.sections[1].groups[1];
        assert_eq!((stamina.description.as_deref(), stamina.author.as_deref(), stamina.version.as_deref()), (Some("From code."), Some("kurozael"), Some("0.93")));
        assert_eq!(stamina.modules[0].id, "Stamina");
        assert_eq!(p.resolve("Kernel.Init").as_deref(), Some("catwork/Kernel.html#Init"));
        assert_eq!(p.resolve("x"), None, "files outside every group are skipped");
    }

    #[test]
    fn resolves_references() {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let p = build(files(), BuildOptions { fallback_title: Some("fallback"), ..BuildOptions::new(&layout, &meta) });
        assert_eq!(p.resolve("Core::A#other").as_deref(), Some("flux/Core.A.html#other"));
        assert_eq!(p.resolve("Core.A:do_x").as_deref(), Some("flux/Core.A.html#do_x"));
        assert_eq!(p.resolve("Core.A").as_deref(), Some("flux/Core.A.html"));
        assert_eq!(p.resolve("g").as_deref(), Some("flux/Globals.html#g"));
        assert_eq!(p.resolve("player_meta#jump").as_deref(), Some("flux/Player.html#jump"));
        assert_eq!(p.resolve("Player:jump()").as_deref(), Some("flux/Player.html#jump"));
        assert_eq!(p.resolve("nothing"), None);
    }
}
