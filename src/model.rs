//! Builds the documentation model (sections, groups, modules, functions)
//! from the scanned files, and the cross-reference index used for links.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::doc::{DocBlock, Environment, Realm};
use crate::layout::{GroupMeta, Layout, Placement};
use crate::scanner::{path_realm, Category, FileScan, FunctionDecl, Sep};

/// Template objects that the loader injects as globals and redefines per
/// file (`PANEL:Init`, `ENT:Think`, ...), so each file becomes its own module,
/// with the definition label of each. Objects declared as file locals are
/// recognised from the declaration instead, see `is_object_init`.
const PER_FILE_OBJECTS: [(&str, &str); 14] = [
    ("PANEL", "Panel"),
    ("CMD", "Command"),
    ("SKIN", "Skin"),
    ("THEME", "Theme"),
    ("TOOL", "Tool"),
    ("ENT", "Entity"),
    ("SWEP", "Weapon"),
    ("EFFECT", "Effect"),
    ("ROLE", "Role"),
    ("PACKAGE", "Package"),
    ("ITEM", "Item"),
    ("ATTRIBUTE", "Attribute"),
    ("FACTION", "Faction"),
    ("CONDITION", "Condition"),
];

/// Template-style local names that make a definition besides those of
/// `PER_FILE_OBJECTS`, such as `local COMMAND = cw.command:New('A')`.
const LOCAL_DEFINITIONS: [(&str, &str); 2] = [("COMMAND", "Command"), ("CLASS", "Class")];

/// Plural definition labels in the order they are listed in; others follow
/// alphabetically.
const DEFINITION_ORDER: [&str; 15] = ["Commands", "Items", "Factions", "Classes", "Roles", "Attributes", "Conditions", "Entities", "Weapons", "Effects", "Tools", "Panels", "Themes", "Skins", "Packages"];

/// File names of an entity, weapon or effect folder; the files of one folder
/// form one module.
const ENTITY_FILES: [&str; 3] = ["init.lua", "cl_init.lua", "shared.lua"];

/// Fields that give an object its display title, in order of preference.
const TITLE_FIELDS: [&str; 4] = ["PrintName", "name", "Name", "title"];

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
    /// The hooks a group runs (`hook.Run`, `hook.Call`, `Plugin.call`, ...),
    /// one function per hook name.
    Hooks,
    /// A table whose methods handle hooks. `object` is `GM`, `PLUGIN` (a
    /// plugin, under its global alias when it has one), `Schema`, or
    /// `hook.Add` for the handlers added by files without such a table.
    Handlers { object: String },
    Globals,
    /// A per-file template object such as a `PANEL`, or a file-local object
    /// table; `object` is the name it is written as in the code.
    Object { object: String },
}

/// The definition label of a per-file template object (`CMD` -> `Command`).
fn per_file_label(object: &str) -> Option<&'static str> {
    PER_FILE_OBJECTS.iter().find(|(o, _)| *o == object).map(|(_, label)| *label)
}

/// The plural of a definition label: `Entity` -> `Entities`, `Class` ->
/// `Classes`, `Command` -> `Commands`.
fn plural(label: &str) -> String {
    if let Some(stem) = label.strip_suffix('y').filter(|s| !s.ends_with(['a', 'e', 'i', 'o', 'u'])) {
        format!("{stem}ies")
    } else if label.ends_with('s') || label.ends_with('x') || label.ends_with("ch") || label.ends_with("sh") {
        format!("{label}es")
    } else {
        format!("{label}s")
    }
}

/// Sort key of a plural definition label: the known labels in
/// `DEFINITION_ORDER`, then the others alphabetically.
fn definition_rank(plural: &str) -> (usize, String) {
    let known = DEFINITION_ORDER.iter().position(|p| *p == plural).unwrap_or(DEFINITION_ORDER.len());
    (known, plural.to_ascii_lowercase())
}

impl ModuleKind {
    pub fn badge(&self) -> &str {
        match self {
            ModuleKind::Class { .. } => "class",
            ModuleKind::Library => "library",
            ModuleKind::Hooks => "hooks",
            ModuleKind::Handlers { object } => match object.as_str() {
                "GM" => "gamemode",
                "Schema" => "schema",
                "hook.Add" => "handlers",
                _ => "plugin",
            },
            ModuleKind::Globals => "globals",
            ModuleKind::Object { .. } => "object",
        }
    }
}

/// Where a `Function` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// A `function` declaration or `name = function` assignment.
    Declared,
    /// A hook the code runs, built from its call sites (in a `Hooks` module).
    Hook,
    /// A handler added with `hook.Add`.
    HookAdd,
}

/// The definition of a branch twin in the `else` arm of the `if` statement
/// whose first arm holds the primary definition.
#[derive(Debug, Clone)]
pub struct Otherwise {
    /// Source of the first arm's condition, such as `is_development`.
    pub condition: String,
    pub doc: Option<DocBlock>,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    /// Display owner, e.g. `ActiveRecord.Base`; empty for globals, hooks and
    /// `hook.Add` handlers.
    pub owner: String,
    pub sep: Sep,
    pub params: Vec<String>,
    pub doc: Option<DocBlock>,
    /// Primary location: the shared definition, else the server one.
    pub file: String,
    pub line: usize,
    pub anchor: String,
    pub category: String,
    pub kind: FunctionKind,
    /// Where it runs: the `@realm` tag, else the enclosing `if SERVER` /
    /// `if CLIENT` block, else the file path. Server and client twins merged
    /// into one function are `Shared`.
    pub realm: Realm,
    /// Every definition (file, line, realm), the primary one first. More
    /// than one for merged realm and branch twins; every call site for a
    /// hook.
    pub sources: Vec<(String, usize, Realm)>,
    /// Realm of the definition `doc` comes from. For merged twins it can
    /// differ from the primary definition's realm, when that one is not
    /// documented.
    pub doc_realm: Realm,
    /// The client definition's doc of a merged twin, when it differs from
    /// `doc`.
    pub client_doc: Option<DocBlock>,
    /// The `else` arm definition of a branch twin: a function defined in
    /// both arms of one `if` statement.
    pub otherwise: Option<Otherwise>,
    /// For hooks: the functions that run it, qualified as they are
    /// documented (`Player:GetData`, `cw.core:Initialize`, `helper`).
    pub callers: Vec<String>,
    /// For `hook.Add` handlers: the identifier, when it is a string literal.
    pub hook_id: Option<String>,
    /// For functions in the `Hooks` category: the reference key (`hook:Name`)
    /// of the hook of that name the project runs, if any.
    pub implements: Option<String>,
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
    /// What the module defines when it is a definition rather than code:
    /// a template object or file-local object that the framework loads
    /// from its own file, such as a `Command` or an `Entity`.
    pub definition: Option<String>,
}

impl Module {
    /// The badge text: the definition label in lower case (`command`),
    /// else the kind's badge.
    pub fn badge(&self) -> String {
        match &self.definition {
            Some(label) => label.to_lowercase(),
            None => self.kind.badge().to_string(),
        }
    }

    /// Functions grouped by category, in category order.
    pub fn grouped(&self) -> Vec<(&Category, Vec<&Function>)> {
        self.categories.iter().map(|c| (c, self.functions.iter().filter(|f| f.category == c.name).collect::<Vec<_>>())).filter(|(_, fs)| !fs.is_empty()).collect()
    }

    pub fn summary(&self) -> String {
        self.doc.as_ref().map(|d| d.summary()).unwrap_or_default()
    }

    /// The environment the module's doc limits it to (`@environment` in
    /// the file doc).
    pub fn environment(&self) -> Option<&Environment> {
        self.doc.as_ref().and_then(|d| d.environment.as_ref())
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

impl Group {
    /// The modules that are code rather than definitions, in module order.
    pub fn code_modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.iter().filter(|m| m.definition.is_none())
    }

    /// The definition modules by plural label (`Commands`), in definition
    /// order, each sorted by title, then id.
    pub fn definitions(&self) -> Vec<(String, Vec<&Module>)> {
        let mut kinds: BTreeMap<(usize, String), (String, Vec<&Module>)> = BTreeMap::new();
        for m in &self.modules {
            if let Some(label) = &m.definition {
                let label = plural(label);
                kinds.entry(definition_rank(&label)).or_insert_with(|| (label, Vec::new())).1.push(m);
            }
        }
        kinds
            .into_values()
            .map(|(label, mut modules)| {
                modules.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
                (label, modules)
            })
            .collect()
    }
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
    /// Metadata from `plugin.ini` files, by the group's source directory
    /// (`Placement::source_dir`).
    pub group_meta: &'a HashMap<String, GroupMeta>,
}

impl<'a> BuildOptions<'a> {
    /// Options with nothing set apart from the layout and group metadata.
    pub fn new(layout: &'a Layout, group_meta: &'a HashMap<String, GroupMeta>) -> BuildOptions<'a> {
        BuildOptions { title: None, fallback_title: None, version: None, summary: None, description: None, documented_only: false, source_url: None, layout, group_meta }
    }
}

/// The last component of a `/` separated path without its `.lua` extension.
pub fn file_stem(path: &str) -> &str {
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_suffix(".lua").unwrap_or(last)
}

/// The file stem without its `sv_`, `cl_` or `sh_` realm prefix.
fn bare_stem(path: &str) -> String {
    let stem = file_stem(path);
    for prefix in ["sv_", "cl_", "sh_"] {
        if let Some(rest) = stem.strip_prefix(prefix).filter(|r| !r.is_empty()) {
            return rest.to_string();
        }
    }
    stem.to_string()
}

/// The name of the directory a file is in, if any.
fn parent_dir(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    parts.next();
    parts.next()
}

pub fn slugify(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect()
}

struct GroupBuilder {
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    version: Option<String>,
    plugin_global: Option<String>,
    /// The group's directory in the source tree; module subtitles show file
    /// paths relative to it.
    source_dir: String,
    files: Vec<(String, FileScan)>,
}

/// A scanned source file: its path relative to the source root, the group
/// of the layout it belongs to, and what it declares.
pub type PlacedFile = (String, Placement, FileScan);

/// The first value of `key` in the root `packagespec.lua`.
fn root_spec(files: &[PlacedFile], key: &str) -> Option<String> {
    files.iter().filter(|(path, _, _)| path == "packagespec.lua").flat_map(|(_, _, scan)| &scan.spec).find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// The project name: `title`, else the root packagespec's name, else
/// `fallback`.
pub fn project_title(files: &[PlacedFile], title: Option<&str>, fallback: Option<&str>) -> String {
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

/// Places each file in its group of `layout`, leaving out the files that
/// match no group.
#[cfg(test)]
pub fn placed(layout: &Layout, files: Vec<(String, FileScan)>) -> Vec<PlacedFile> {
    files.into_iter().filter_map(|(path, scan)| layout.classify(&path).map(|place| (path, place, scan))).collect()
}

/// Builds the project from the files, each placed in a group of
/// `opts.layout`.
pub fn build(files: Vec<PlacedFile>, opts: BuildOptions) -> Project {
    let title = project_title(&files, opts.title, opts.fallback_title);
    let version = opts.version.map(str::to_string).or_else(|| root_spec(&files, "version"));
    let summary = opts.summary.map(str::to_string).or_else(|| root_spec(&files, "summary"));
    let description = opts.description.map(str::to_string).or_else(|| root_spec(&files, "description"));
    let layout = opts.layout;

    // Gather files per group, keyed by section, spec family and key, so
    // that groups of different specs are never merged. `PLUGIN:set_*`
    // calls and `PLUGIN.name = ...` fields fill the builder; the group's own
    // packagespec is kept aside.
    let mut groups: BTreeMap<(usize, usize, String), (Placement, GroupMeta, GroupBuilder)> = BTreeMap::new();
    let mut called: HashSet<String> = HashSet::new();
    for (path, place, scan) in files {
        let (place, spec, g) = groups.entry((place.section, place.family, place.key.clone())).or_insert_with(|| {
            let g = GroupBuilder { name: None, description: None, author: None, version: None, plugin_global: None, source_dir: place.source_dir.clone(), files: Vec::new() };
            (place, GroupMeta::default(), g)
        });
        let plugin_field = |name: &str| scan.fields.iter().find(|(o, f, _)| o == "PLUGIN" && f == name).map(|(_, _, v)| v.clone());
        if let Some(n) = scan.plugin_name.clone().or_else(|| plugin_field("name")) {
            g.name.get_or_insert(n);
        }
        if let Some(d) = scan.plugin_description.clone().or_else(|| plugin_field("description")) {
            g.description.get_or_insert(d);
        }
        if let Some(a) = scan.plugin_author.clone().or_else(|| plugin_field("author")) {
            g.author.get_or_insert(a);
        }
        if let Some(gl) = &scan.plugin_global {
            g.plugin_global.get_or_insert(gl.clone());
        }
        if !place.core && path == format!("{}/packagespec.lua", place.source_dir) {
            *spec = package_meta(&scan);
        }
        called.extend(scan.hook_calls.iter().map(|c| c.name.clone()));
        g.files.push((path, scan));
    }

    let mut sections: Vec<Section> = layout.sections.iter().map(|s| Section { title: s.title.clone(), groups: Vec::new() }).collect();
    let mut placed: Vec<(Placement, Group)> = Vec::new();
    let mut dirs: HashSet<String> = HashSet::new();
    // Groups of specs without a `*` keep their directory when one made by a
    // `*` collides with it.
    let mut groups: Vec<(Placement, GroupMeta, GroupBuilder)> = groups.into_values().collect();
    groups.sort_by_key(|(place, _, _)| layout.group(place).is_pattern());
    for (place, spec, mut gb) in groups {
        // Name priority: the layout, plugin.ini, packagespec, `PLUGIN:set_name`,
        // the path. Core groups are named after their section instead.
        let fixed = layout.group(&place).name.clone();
        let ini = opts.group_meta.get(&place.source_dir).cloned().unwrap_or_default();
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

        let modules = build_modules(&gb, &group_title, opts.documented_only, &called);
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
    placed.sort_by(|(a, ga), (b, gb)| b.core.cmp(&a.core).then_with(|| a.key.to_ascii_lowercase().cmp(&b.key.to_ascii_lowercase())).then_with(|| ga.dir.cmp(&gb.dir)));
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

/// Realm of a definition: its `@realm` tag, else the enclosing `if SERVER`
/// / `if CLIENT` block, else the file path.
fn definition_realm(doc: Option<&DocBlock>, block: Realm, path: &str) -> Realm {
    doc.and_then(|d| d.realm).unwrap_or_else(|| match block {
        Realm::Shared => path_realm(path),
        specific => specific,
    })
}

/// Which definition of realm twins is the primary one: shared, then server.
fn realm_rank(realm: Realm) -> u8 {
    match realm {
        Realm::Shared => 0,
        Realm::Server => 1,
        Realm::Client => 2,
    }
}

/// True when a method name looks like a hook: it starts with an upper case
/// letter and is not all upper case (`PlayerSpawn`, not `pickup` or `OK`).
fn is_hook_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && name.chars().any(|c| c.is_ascii_lowercase())
}

/// True when the right-hand side of a `local` creates an object: a table
/// constructor, `setmetatable(...)`, a method call such as
/// `cw.command:New('A')`, or a constructor-like call such as `item.New(...)`.
/// Aliases (`PLUGIN`), field paths with defaults (`cw.currency.stored or {}`),
/// literals and other expressions do not.
fn is_object_init(init: &str) -> bool {
    let init = init.trim();
    if init.starts_with('{') {
        return true;
    }
    let Some((path, args)) = split_call(init) else { return false };
    if !is_whole_call(args) {
        return false;
    }
    if path == "setmetatable" {
        return true;
    }
    match path.rfind(['.', ':']) {
        Some(p) if path[p..].starts_with(':') => true,
        Some(p) => is_constructor_name(&path[p + 1..]),
        None => false,
    }
}

/// Splits a call such as `a.b:c(x)` into the callee path and the arguments
/// from `(` on.
fn split_call(init: &str) -> Option<(&str, &str)> {
    let end = init.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':')))?;
    let (path, args) = init.split_at(end);
    let first = path.chars().next()?;
    ((first.is_ascii_alphabetic() || first == '_') && !path.ends_with(['.', ':']) && args.starts_with('(')).then_some((path, args))
}

/// True when `args` (from `(` on) is a single argument list with nothing
/// after it, or is cut off by the scanner's length limit.
fn is_whole_call(args: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in args.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '\'' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '\'' => in_string = true,
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return args[i + c.len_utf8()..].trim().is_empty();
                }
            }
            _ => {}
        }
    }
    args.ends_with('…')
}

fn is_constructor_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "register" | "begin" | "define" | "extend" | "class") || n.starts_with("new") || n.starts_with("create")
}

/// The first argument of a constructor call when it is a string literal
/// other than a language key: `cw.command:New('A')` -> `A`, but not
/// `faction.New('#Faction_Admin')`.
fn constructor_name(init: &str) -> Option<String> {
    let (_, args) = split_call(init.trim())?;
    let rest = args.strip_prefix("('")?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '\'' => return Some(out).filter(|s| !s.trim().is_empty() && !s.starts_with('#')),
            c => out.push(c),
        }
    }
    None
}

/// A reference-safe module id for a name: names with whitespace are
/// joined in CamelCase (`Manage Players` -> `ManagePlayers`), other names
/// are kept.
fn name_id(name: &str) -> String {
    if !name.contains(char::is_whitespace) {
        return name.to_string();
    }
    name.split_whitespace().map(capitalize).collect()
}

/// `s` with its first character in upper case.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The display title an object's fields give it (`ENT.PrintName = 'Item'`),
/// on one line, skipping language keys such as `#Item_Name`.
fn title_field(scan: &FileScan, object: &str) -> Option<String> {
    TITLE_FIELDS.iter().find_map(|field| {
        scan.fields.iter().find(|(o, f, v)| o == object && f == field && !v.trim().is_empty() && !v.starts_with('#')).map(|(_, _, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

/// The object name used in generated ids: `CLASS_TABLE` -> `Class`,
/// `PANEL` -> `Panel`, `ITEM_META` -> `ItemMeta`, `stored` -> `Stored`.
fn object_word(local: &str) -> String {
    if local.chars().any(|c| c.is_ascii_lowercase()) {
        return capitalize(local);
    }
    let name = local.strip_suffix("_TABLE").filter(|n| !n.is_empty()).unwrap_or(local);
    name.split('_').map(|part| capitalize(&part.to_ascii_lowercase())).collect()
}

/// The definition label of a file-local object, if it is a definition: a
/// template-style local named after a template (`local PANEL = {}`,
/// `local COMMAND = ...`), else one created by a constructor call on a
/// library, labelled after the library (`cw.system:New('A')` -> `System`).
/// Other tables, `setmetatable(...)` and other calls are code.
fn local_definition(name: &str, init: &str) -> Option<String> {
    if name.chars().any(|c| c.is_ascii_lowercase()) {
        return None;
    }
    if let Some(label) = per_file_label(name).or_else(|| LOCAL_DEFINITIONS.iter().find(|(o, _)| *o == name).map(|(_, label)| *label)) {
        return Some(label.to_string());
    }
    let (path, args) = split_call(init.trim())?;
    let p = path.rfind(['.', ':'])?;
    if !is_whole_call(args) || !is_constructor_name(&path[p + 1..]) {
        return None;
    }
    let library = path[..p].rsplit('.').next()?;
    if library.is_empty() || !library.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    // `local BLUEPRINT = cw.blueprints:New()` is a `Blueprint`.
    let own = name.to_ascii_lowercase();
    let word = if library.strip_suffix('s') == Some(own.as_str()) { own.as_str() } else { library };
    Some(capitalize(word))
}

/// True when the descriptions of two doc blocks match, ignoring `@realm`
/// and `@category`.
fn same_text(a: &DocBlock, b: &DocBlock) -> bool {
    let strip = |d: &DocBlock| DocBlock { realm: None, category: None, ..d.clone() };
    strip(a) == strip(b)
}

/// True when a doc block cannot document a module: its first line has no
/// letters, such as an ASCII-art banner.
fn is_banner(doc: &DocBlock) -> bool {
    doc.description.lines().map(str::trim).find(|l| !l.is_empty()).is_none_or(|l| !l.chars().any(|c| c.is_ascii_alphabetic()))
}

/// The module a declaration lands in, and how to create it.
#[derive(Clone)]
struct Target {
    /// Identity of the module inside the group.
    key: String,
    id: String,
    title: String,
    subtitle: Option<String>,
    kind: ModuleKind,
    doc: Option<DocBlock>,
    decl_file: Option<(String, usize)>,
    definition: Option<String>,
}

impl Target {
    fn new(key: String, id: String, kind: ModuleKind) -> Target {
        Target { key, title: id.clone(), id, subtitle: None, kind, doc: None, decl_file: None, definition: None }
    }

    fn into_module(self) -> Module {
        let mut m = new_module(&self.id, self.kind);
        m.title = self.title;
        m.subtitle = self.subtitle;
        m.doc = self.doc;
        m.decl_file = self.decl_file;
        m.definition = self.definition;
        m
    }
}

const HOOKS_KEY: &str = "\u{3}hooks";

/// The handler module of `owner`; `object` is `GM`, `PLUGIN`, `Schema` or
/// `hook.Add`.
fn handler_target(owner: &str, object: &str, group_title: &str) -> Target {
    let subtitle = match object {
        "GM" => "Gamemode hooks".to_string(),
        "Schema" => "Schema hooks and methods".to_string(),
        "hook.Add" => format!("Hooks added by {group_title}"),
        _ => "Plugin hooks and methods".to_string(),
    };
    let mut t = Target::new(format!("\u{1}handler\u{0}{}", owner.to_ascii_lowercase()), owner.to_string(), ModuleKind::Handlers { object: object.to_string() });
    t.subtitle = Some(subtitle);
    t
}

/// The handler object an owner stands for, if it is a hook table.
fn handler_object(raw_owner: &str, owner: &str, plugin_global: &Option<String>, plugin_tables: &HashSet<String>) -> Option<&'static str> {
    if owner == "GM" {
        Some("GM")
    } else if owner == "Schema" {
        Some("Schema")
    } else if raw_owner == "PLUGIN" || plugin_global.as_deref() == Some(owner) || plugin_tables.contains(owner) {
        Some("PLUGIN")
    } else {
        None
    }
}

/// Global tables of a group that look like plugin tables: they are given an
/// `author` field.
fn plugin_tables(gb: &GroupBuilder) -> HashSet<String> {
    gb.files
        .iter()
        .flat_map(|(_, scan)| &scan.fields)
        .filter(|(owner, field, _)| field == "author" && !owner.contains('.') && per_file_label(owner).is_none())
        .map(|(owner, _, _)| owner.clone())
        .collect()
}

/// True when a file refers to its plugin: `local PLUGIN = PLUGIN`,
/// `PLUGIN:set_name(...)` or `PLUGIN.field = ...`.
fn file_has_plugin(scan: &FileScan) -> bool {
    scan.locals.iter().any(|l| l.name == "PLUGIN")
        || scan.plugin_name.is_some()
        || scan.plugin_description.is_some()
        || scan.plugin_author.is_some()
        || scan.plugin_global.is_some()
        || scan.fields.iter().any(|(owner, _, _)| owner == "PLUGIN")
}

/// The modules of a file's object locals, by local index: locals whose
/// initialiser creates an object (see `is_object_init`) and that functions
/// are defined on. Objects are named by `@module`, `vgui.Register`, the
/// constructor's string argument, else a template-style local (`PANEL`,
/// `CLASS_TABLE`) gets an id from the file's library or name
/// (`cw.currency.Class`) and any other local keeps its own name.
fn local_objects(path: &str, rel: &str, scan: &FileScan, documented_only: bool) -> Vec<Option<Target>> {
    let used: BTreeSet<usize> = scan
        .functions
        .iter()
        .filter(|f| !documented_only || f.doc.is_some())
        .filter_map(|f| f.local_table)
        .filter(|&i| is_object_init(&scan.locals[i].init))
        .collect();
    let single = used.len() == 1 && scan.libraries.is_empty();
    let mut out = vec![None; scan.locals.len()];
    for &i in &used {
        let local = &scan.locals[i];
        let file_module = if single { scan.file_doc.as_ref().and_then(|d| d.module.clone()) } else { None };
        let named = local.doc.as_ref().and_then(|d| d.module.clone()).or(file_module).or_else(|| local.register_name.clone()).or_else(|| constructor_name(&local.init));
        let unique = scan.locals.iter().filter(|l| l.name == local.name).count() == 1;
        let field_title = || unique.then(|| title_field(scan, &local.name)).flatten();
        let template = !local.name.chars().any(|c| c.is_ascii_lowercase());
        let (id, title) = match named {
            Some(name) => (name_id(&name), name),
            None if !template => (local.name.clone(), field_title().unwrap_or_else(|| local.name.clone())),
            None => {
                let word = object_word(&local.name);
                let id = match scan.libraries.first() {
                    Some((library, _)) => format!("{library}.{word}"),
                    None => format!("{}.{word}", bare_stem(path)),
                };
                let title = field_title().unwrap_or_else(|| id.clone());
                (id, title)
            }
        };
        let definition = local_definition(&local.name, &local.init);
        let subtitle = match &definition {
            Some(label) => format!("{label} defined in {rel}"),
            None => format!("Object table {} in {rel}", local.name),
        };
        out[i] = Some(Target {
            key: format!("\u{2}object\u{0}{path}\u{0}{i:06}"),
            id,
            title,
            subtitle: Some(subtitle),
            kind: ModuleKind::Object { object: local.name.clone() },
            doc: local.doc.clone(),
            decl_file: Some((path.to_string(), local.line)),
            definition,
        });
    }
    out
}

/// The module a function on `owner` (already resolved) lands in when it is
/// not defined on an object local.
fn owner_target(gb: &GroupBuilder, group_title: &str, path: &str, scan: &FileScan, f: &FunctionDecl, owner: &str, plugin_tables: &HashSet<String>) -> Target {
    let rel = rel_path(gb, path);
    let first = owner.split('.').next().unwrap_or("");
    if owner.is_empty() {
        let mut t = Target::new("globals".to_string(), "Globals".to_string(), ModuleKind::Globals);
        t.subtitle = Some(format!("Global functions of {group_title}"));
        return t;
    }
    if let Some(label) = per_file_label(first) {
        let kind = ModuleKind::Object { object: first.to_string() };
        let file = path.rsplit('/').next().unwrap_or(path);
        if ENTITY_FILES.contains(&file)
            && let Some(dir) = parent_dir(path)
        {
            let folder = &path[..path.len() - file.len() - 1];
            let mut t = Target::new(format!("\u{2}entity\u{0}{first}\u{0}{folder}"), dir.to_string(), kind);
            t.title = title_field(scan, first).unwrap_or_else(|| dir.to_string());
            t.subtitle = Some(format!("{label} defined in {}", rel_path(gb, folder)));
            t.definition = Some(label.to_string());
            return t;
        }
        let id = if first == "PANEL" { scan.vgui_name.clone().unwrap_or_else(|| file_stem(path).to_string()) } else { file_stem(path).to_string() };
        let mut t = Target::new(format!("\u{2}file\u{0}{first}\u{0}{path}"), id.clone(), kind);
        t.title = title_field(scan, first).unwrap_or(id);
        t.subtitle = Some(format!("{label} defined in {rel}"));
        t.definition = Some(label.to_string());
        return t;
    }
    if let Some(object) = handler_object(&f.owner, owner, &gb.plugin_global, plugin_tables) {
        return handler_target(owner, object, group_title);
    }
    Target::new(owner.to_ascii_lowercase(), owner.to_string(), ModuleKind::Library)
}

/// A path relative to the group's source directory.
fn rel_path(gb: &GroupBuilder, path: &str) -> String {
    path.strip_prefix(gb.source_dir.as_str()).and_then(|p| p.strip_prefix('/')).unwrap_or(path).to_string()
}

/// Adds the category to the module unless it has it, with the description
/// the file gives it.
fn ensure_category(m: &mut Module, name: &str, file_categories: &HashMap<&str, &Category>) {
    if !m.categories.iter().any(|c| c.name == name) {
        let description = file_categories.get(name).map(|c| c.description.clone()).unwrap_or_default();
        m.categories.push(Category { name: name.to_string(), description });
    }
}

/// Adds a declared function, merging it into a realm twin: a function of
/// the same name defined for another realm.
fn add_function(m: &mut Module, f: Function) {
    let twin = m.functions.iter().position(|e| e.kind == FunctionKind::Declared && e.name == f.name && e.sep == f.sep && !e.sources.iter().any(|s| s.2 == f.realm));
    match twin {
        Some(i) => merge_twin(&mut m.functions[i], f),
        None => m.functions.push(f),
    }
}

/// Merges `f` into its twin `e`. The shared, else the server definition
/// is the primary one. The doc of the shared, else the server, else the
/// client definition wins, and a differing client doc is kept as
/// `client_doc`.
fn merge_twin(e: &mut Function, mut f: Function) {
    if realm_rank(f.realm) < realm_rank(e.sources[0].2) {
        std::mem::swap(e, &mut f);
    }
    let candidates = [(e.doc_realm, e.doc.take()), (Realm::Client, e.client_doc.take()), (f.doc_realm, f.doc.take()), (Realm::Client, f.client_doc.take())];
    let mut docs: Vec<(Realm, DocBlock)> = candidates.into_iter().filter_map(|(r, d)| d.map(|d| (r, d))).collect();
    docs.sort_by_key(|(r, _)| realm_rank(*r));
    let mut docs = docs.into_iter();
    let (doc_realm, doc) = docs.next().map_or((e.sources[0].2, None), |(r, d)| (r, Some(d)));
    e.doc_realm = doc_realm;
    e.doc = doc;
    e.client_doc = docs.find(|(r, d)| *r == Realm::Client && !e.doc.as_ref().is_some_and(|p| same_text(p, d))).map(|(_, d)| d);
    e.sources.extend(f.sources);
    e.otherwise = e.otherwise.take().or(f.otherwise);
    e.realm = Realm::Shared;
}

/// Pairs the branch twins of a file: a function declared in the first arm
/// of an `if` statement and again, under the same name and for the same
/// realm, in one of its `elseif` / `else` arms. Maps the index of the first
/// arm's declaration to the other's.
fn branch_twins(path: &str, scan: &FileScan) -> HashMap<usize, usize> {
    let mut twins = HashMap::new();
    let mut taken = HashSet::new();
    let realm = |f: &FunctionDecl| definition_realm(f.doc.as_ref(), f.realm, path);
    for (i, f) in scan.functions.iter().enumerate() {
        let Some(branch) = f.branch.as_ref().filter(|b| !b.else_branch) else { continue };
        let twin = scan.functions.iter().enumerate().skip(i + 1).find(|(k, g)| {
            !taken.contains(k)
                && g.name == f.name
                && g.sep == f.sep
                && g.owner == f.owner
                && g.local_table == f.local_table
                && g.branch.as_ref().is_some_and(|b| b.if_id == branch.if_id && b.else_branch)
                && realm(g) == realm(f)
        });
        if let Some((k, _)) = twin {
            taken.insert(k);
            twins.insert(i, k);
        }
    }
    twins
}

/// The doc of the `GM`, else the `Schema` handler of a hook, for a hook
/// whose call sites are not documented. Plugin handlers document what the
/// plugin does, not the hook.
fn handler_doc(modules: &BTreeMap<String, Module>, hook: &str) -> Option<DocBlock> {
    ["GM", "Schema"].iter().find_map(|object| {
        modules
            .values()
            .filter(|m| matches!(&m.kind, ModuleKind::Handlers { object: o } if o == object))
            .flat_map(|m| &m.functions)
            .find(|f| f.kind == FunctionKind::Declared && f.name == hook && f.doc.is_some())
            .and_then(|f| f.doc.clone())
    })
}

/// Module order inside a group: the hooks page, hook handlers, globals,
/// then everything else.
fn module_rank(kind: &ModuleKind) -> u8 {
    match kind {
        ModuleKind::Hooks => 0,
        ModuleKind::Handlers { .. } => 1,
        ModuleKind::Globals => 2,
        _ => 3,
    }
}

fn build_modules(gb: &GroupBuilder, group_title: &str, documented_only: bool, called: &HashSet<String>) -> Vec<Module> {
    // Module key -> module. Object modules are keyed by their file.
    let mut modules: BTreeMap<String, Module> = BTreeMap::new();
    let plugin_tables = plugin_tables(gb);

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

    // (file, qualified name as written) -> (module key, sep, name), to name
    // the callers of hooks.
    let mut declared: HashMap<(&str, String), (String, Sep, String)> = HashMap::new();
    // Module keys each file's functions land in, its object modules and its
    // hook handler module.
    let mut file_modules: HashMap<&str, Vec<String>> = HashMap::new();
    let mut file_objects: HashMap<&str, Vec<String>> = HashMap::new();
    let mut file_handler: HashMap<&str, String> = HashMap::new();

    for (path, scan) in &gb.files {
        let file_categories: HashMap<&str, &Category> = scan.categories.iter().map(|c| (c.name.as_str(), c)).collect();
        let objects = local_objects(path, &rel_path(gb, path), scan, documented_only);
        file_objects.insert(path, objects.iter().flatten().map(|t| t.key.clone()).collect());

        let twins = branch_twins(path, scan);
        let others: HashSet<usize> = twins.values().copied().collect();
        for (index, f) in scan.functions.iter().enumerate() {
            if others.contains(&index) {
                continue;
            }
            let other = twins.get(&index).map(|&k| &scan.functions[k]);
            if documented_only && f.doc.is_none() && other.is_none_or(|o| o.doc.is_none()) {
                continue;
            }
            let owner = resolve_owner(&f.owner, scan, &gb.plugin_global);
            let target = match f.local_table.and_then(|i| objects[i].clone()) {
                Some(t) => t,
                None => owner_target(gb, group_title, path, scan, f, &owner, &plugin_tables),
            };
            let key = target.key.clone();
            let title = target.title.clone();
            let m = modules.entry(key.clone()).or_insert_with(|| target.into_module());
            if m.title == m.id && title != m.id {
                m.title = title;
            }
            if !m.files.contains(path) {
                m.files.push(path.clone());
            }
            if matches!(m.kind, ModuleKind::Library) && f.sep == Sep::Colon {
                m.kind = ModuleKind::Class { extends: None };
            }

            let category = f.category.clone().unwrap_or_else(|| match (&m.kind, f.sep) {
                (ModuleKind::Handlers { .. }, _) if is_hook_name(&f.name) => "Hooks".to_string(),
                (_, Sep::Colon) => "Methods".to_string(),
                _ => "Functions".to_string(),
            });
            ensure_category(m, &category, &file_categories);

            let realm = definition_realm(f.doc.as_ref(), f.realm, path);
            let mut sources = vec![(path.clone(), f.line, realm)];
            let otherwise = other.map(|o| {
                sources.push((path.clone(), o.line, realm));
                Otherwise { condition: f.branch.as_ref().map(|b| b.condition.clone()).unwrap_or_default(), doc: o.doc.clone(), file: path.clone(), line: o.line }
            });
            add_function(
                m,
                Function {
                    name: f.name.clone(),
                    owner: owner.clone(),
                    sep: f.sep,
                    params: f.params.clone(),
                    doc: f.doc.clone(),
                    file: path.clone(),
                    line: f.line,
                    anchor: String::new(),
                    category,
                    kind: FunctionKind::Declared,
                    realm,
                    sources,
                    doc_realm: realm,
                    client_doc: None,
                    otherwise,
                    callers: Vec::new(),
                    hook_id: None,
                    implements: None,
                },
            );
            if matches!(m.kind, ModuleKind::Handlers { .. }) {
                file_handler.entry(path).or_insert_with(|| key.clone());
            }
            declared.insert((path, f.qualified()), (key.clone(), f.sep, f.name.clone()));
            let keys = file_modules.entry(path).or_default();
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }

    // `hook.Add` handlers go to the file's plugin or gamemode table, else
    // to a `hook.Add` module.
    for (path, scan) in &gb.files {
        for add in &scan.hook_adds {
            if documented_only && add.doc.is_none() {
                continue;
            }
            let target = if file_has_plugin(scan) { handler_target(gb.plugin_global.as_deref().unwrap_or("PLUGIN"), "PLUGIN", group_title) } else { handler_target("hook.Add", "hook.Add", group_title) };
            let key = file_handler.get(path.as_str()).cloned().unwrap_or_else(|| target.key.clone());
            let m = modules.entry(key).or_insert_with(|| target.into_module());
            if !m.files.contains(path) {
                m.files.push(path.clone());
            }
            ensure_category(m, "Hooks", &HashMap::new());
            let realm = definition_realm(add.doc.as_ref(), add.realm, path);
            m.functions.push(Function {
                name: add.name.clone(),
                owner: String::new(),
                sep: Sep::None,
                params: add.params.clone(),
                doc: add.doc.clone(),
                file: path.clone(),
                line: add.line,
                anchor: String::new(),
                category: "Hooks".to_string(),
                kind: FunctionKind::HookAdd,
                realm,
                sources: vec![(path.clone(), add.line, realm)],
                doc_realm: realm,
                client_doc: None,
                otherwise: None,
                callers: Vec::new(),
                hook_id: add.id.clone(),
                implements: None,
            });
        }
    }

    // The hooks the group runs, one function per name. Callers are kept as
    // `file\0caller` until the module ids are final.
    let mut hook_sites: Vec<(String, Vec<(&str, &crate::scanner::HookCall)>)> = Vec::new();
    for (path, scan) in &gb.files {
        for call in &scan.hook_calls {
            match hook_sites.iter_mut().find(|(name, _)| *name == call.name) {
                Some((_, sites)) => sites.push((path, call)),
                None => hook_sites.push((call.name.clone(), vec![(path, call)])),
            }
        }
    }
    let mut hooks = new_module("Hooks", ModuleKind::Hooks);
    hooks.subtitle = Some(format!("Hooks called by {group_title}"));
    hooks.categories.push(Category { name: "Hooks".to_string(), description: String::new() });
    for (name, sites) in hook_sites {
        let documented = sites.iter().find(|(_, c)| c.doc.is_some());
        let doc = documented.and_then(|(_, c)| c.doc.clone()).or_else(|| handler_doc(&modules, &name));
        if documented_only && doc.is_none() {
            continue;
        }
        let sources: Vec<(String, usize, Realm)> = sites.iter().map(|(p, c)| (p.to_string(), c.line, definition_realm(c.doc.as_ref(), c.realm, p))).collect();
        let realm = if sources.iter().all(|s| s.2 == sources[0].2) { sources[0].2 } else { Realm::Shared };
        let widest = sites.iter().fold(sites[0].1, |best, (_, c)| if c.args.len() > best.args.len() { *c } else { best });
        let (file, primary) = documented.copied().unwrap_or(sites[0]);
        for (p, _) in &sites {
            if !hooks.files.iter().any(|f| f == p) {
                hooks.files.push(p.to_string());
            }
        }
        hooks.functions.push(Function {
            name,
            owner: String::new(),
            sep: Sep::None,
            params: widest.args.clone(),
            doc,
            file: file.to_string(),
            line: primary.line,
            anchor: String::new(),
            category: "Hooks".to_string(),
            kind: FunctionKind::Hook,
            realm,
            sources,
            doc_realm: realm,
            client_doc: None,
            otherwise: None,
            callers: sites.iter().filter_map(|(p, c)| c.caller.as_ref().map(|caller| format!("{p}\u{0}{caller}"))).collect(),
            hook_id: None,
            implements: None,
        });
    }
    if !hooks.functions.is_empty() {
        modules.insert(HOOKS_KEY.to_string(), hooks);
    }

    // File docs document the module their `@module` names, else the
    // module of their library, else their only object or module. Without
    // `@module`, a server or client file documents only a module all of
    // whose functions it defines. Files named `sh_` win, then the first
    // file.
    let mut file_docs: HashMap<String, ((bool, usize), &DocBlock)> = HashMap::new();
    for (order, (path, scan)) in gb.files.iter().enumerate() {
        let Some(doc) = scan.file_doc.as_ref().filter(|d| !is_banner(d)) else { continue };
        let by_id = |id: &str| modules.iter().find(|(_, m)| m.id.eq_ignore_ascii_case(id) && m.kind != ModuleKind::Hooks).map(|(k, _)| k.clone());
        let objects = file_objects.get(path.as_str()).map(Vec::as_slice).unwrap_or_default();
        let single_object = (objects.len() == 1 && scan.libraries.is_empty()).then(|| objects[0].clone());
        let target = match &doc.module {
            Some(name) => single_object.filter(|k| modules.get(k).is_some_and(|m| m.id == name_id(name))).or_else(|| by_id(&name_id(name))),
            None => scan
                .libraries
                .first()
                .and_then(|(library, _)| by_id(library))
                .or(single_object)
                .or_else(|| file_modules.get(path.as_str()).filter(|keys| keys.len() == 1).map(|keys| keys[0].clone()))
                .filter(|key| path_realm(path) == Realm::Shared || modules.get(key).is_some_and(|m| m.functions.iter().flat_map(|f| &f.sources).all(|s| s.0 == *path))),
        };
        if let Some(key) = target {
            let rank = (!file_stem(path).starts_with("sh_"), order);
            if file_docs.get(&key).is_none_or(|(r, _)| rank < *r) {
                file_docs.insert(key, (rank, doc));
            }
        }
    }
    for (key, (_, doc)) in file_docs {
        if let Some(m) = modules.get_mut(&key)
            && m.doc.is_none()
        {
            m.doc = Some(doc.clone());
        }
    }

    // Keep declared classes that have no functions only when documented.
    modules.retain(|_, m| !m.functions.is_empty() || m.doc.is_some());

    for m in modules.values_mut() {
        for f in &mut m.functions {
            if f.category == "Hooks" && f.kind != FunctionKind::Hook && called.contains(&f.name) {
                f.implements = Some(format!("hook:{}", f.name));
            }
        }
    }

    // Ids must be unique in the group. Named modules and the hooks page
    // keep their id before objects do; a colliding module is told apart by
    // its file name or directory, a counter only as the last resort.
    let mut out: Vec<(String, Module)> = modules.into_iter().collect();
    out.sort_by_key(|(key, _)| key.starts_with('\u{2}'));
    // Taken id (lower case) -> file of the object module holding it; a
    // disambiguator must differ from that file's.
    let mut taken: HashMap<String, String> = HashMap::new();
    for (_, m) in &mut out {
        let file = m.files.first().cloned().unwrap_or_default();
        let holder = match taken.get(&m.id.to_ascii_lowercase()) {
            None => {
                let own = if matches!(m.kind, ModuleKind::Object { .. }) { file } else { String::new() };
                taken.insert(m.id.to_ascii_lowercase(), own);
                continue;
            }
            Some(holder) => holder.clone(),
        };
        let parent = |p: &str| parent_dir(p).unwrap_or("").to_string();
        let candidates = [(bare_stem(&file), bare_stem(&holder)), (parent(&file), parent(&holder)), (file_stem(&file).to_string(), file_stem(&holder).to_string())];
        let readable = candidates
            .into_iter()
            .filter(|(d, theirs)| !d.is_empty() && d != theirs && !d.eq_ignore_ascii_case(&m.id))
            .map(|(d, _)| d)
            .find(|d| !taken.contains_key(&format!("{}-{d}", m.id).to_ascii_lowercase()));
        let suffix = readable.unwrap_or_else(|| {
            let mut n = 2;
            while taken.contains_key(&format!("{}-{n}", m.id).to_ascii_lowercase()) {
                n += 1;
            }
            n.to_string()
        });
        taken.insert(format!("{}-{suffix}", m.id).to_ascii_lowercase(), file);
        m.title = format!("{} ({suffix})", m.title);
        m.id = format!("{}-{suffix}", m.id);
    }
    // Slugs must be unique on case-insensitive file systems.
    let mut slugs: HashSet<String> = HashSet::new();
    for (_, m) in &mut out {
        let base = slugify(&m.id);
        let mut slug = base.clone();
        let mut n = 1;
        while !slugs.insert(slug.to_ascii_lowercase()) {
            n += 1;
            slug = format!("{base}-{n}");
        }
        m.slug = slug;
    }

    // Hook callers, named after the modules their functions landed in.
    // Functions on a file's locals or on `self` fields cannot be
    // referenced and are dropped; their call sites stay in `sources`.
    let ids: HashMap<String, (String, bool)> = out.iter().map(|(k, m)| (k.clone(), (m.id.clone(), m.kind == ModuleKind::Globals))).collect();
    let file_locals: HashMap<&str, HashSet<&str>> = gb.files.iter().map(|(path, scan)| (path.as_str(), scan.locals.iter().map(|l| l.name.as_str()).chain(scan.local_names.iter().map(String::as_str)).collect())).collect();
    for (_, m) in &mut out {
        for f in m.functions.iter_mut().filter(|f| f.kind == FunctionKind::Hook) {
            let mut callers: Vec<String> = Vec::new();
            for raw in std::mem::take(&mut f.callers) {
                let (path, caller) = raw.split_once('\u{0}').unwrap_or(("", raw.as_str()));
                let named = match declared.get(&(path, caller.to_string())).and_then(|(key, sep, name)| ids.get(key).map(|(id, globals)| (id, *globals, *sep, name))) {
                    Some((_, true, _, name)) | Some((_, _, Sep::None, name)) => name.clone(),
                    Some((id, _, Sep::Dot, name)) => format!("{id}.{name}"),
                    Some((id, _, Sep::Colon, name)) => format!("{id}:{name}"),
                    None if is_local_caller(caller, file_locals.get(path)) => continue,
                    None => caller.to_string(),
                };
                if !callers.contains(&named) {
                    callers.push(named);
                }
            }
            f.callers = callers;
        }
    }

    let mut out: Vec<Module> = out.into_iter().map(|(_, m)| m).collect();
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

    out.sort_by(|a, b| module_rank(&a.kind).cmp(&module_rank(&b.kind)).then(a.id.to_ascii_lowercase().cmp(&b.id.to_ascii_lowercase())));
    out
}

/// True when a caller is a function on a local or on a `self` field
/// (`label.DoClick`, `self.spawnIcon.DoClick`), so it cannot be referenced.
fn is_local_caller(caller: &str, locals: Option<&HashSet<&str>>) -> bool {
    let Some(end) = caller.rfind(['.', ':']) else { return false };
    let first = caller[..end].split(['.', ':']).next().unwrap_or("");
    first == "self" || locals.is_some_and(|l| l.contains(first))
}

fn new_module(id: &str, kind: ModuleKind) -> Module {
    Module { id: id.to_string(), title: id.to_string(), subtitle: None, slug: slugify(id), kind, doc: None, categories: Vec::new(), functions: Vec::new(), files: Vec::new(), decl_file: None, definition: None }
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
                let spaced_title = m.title.contains(char::is_whitespace).then_some(m.title.as_str());
                if let Some(title) = spaced_title {
                    insert(title.to_string(), &page);
                }
                insert(m.id.replace('.', "::"), &page);
                if let ModuleKind::Object { object } = &m.kind {
                    insert(format!("{object}:{}", m.id), &page);
                }
                // Declared functions take their names before hook handlers.
                let mut functions: Vec<&Function> = m.functions.iter().collect();
                functions.sort_by_key(|f| f.kind != FunctionKind::Declared);
                for f in functions {
                    let url = format!("{page}#{}", f.anchor);
                    match f.kind {
                        FunctionKind::Hook => {
                            insert(format!("hook:{}", f.name), &url);
                            for owner in ["Hooks", m.id.as_str()] {
                                for sep in ['#', ':', '.'] {
                                    insert(format!("{owner}{sep}{}", f.name), &url);
                                }
                            }
                            continue;
                        }
                        FunctionKind::HookAdd => {
                            insert(format!("{}#{}", m.id, f.name), &url);
                            continue;
                        }
                        FunctionKind::Declared => {}
                    }
                    if f.owner.is_empty() {
                        insert(f.name.clone(), &url);
                        continue;
                    }
                    let owners = [Some(f.owner.clone()), Some(f.owner.replace('.', "::")), Some(m.id.clone()), spaced_title.map(str::to_string)];
                    for owner in owners.iter().flatten().collect::<HashSet<_>>() {
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
    /// Resolves a reference such as `Owner#name`, `Owner.name`, `name`,
    /// `Owner` or `hook:Name` to a URL relative to the docs root. A
    /// reference with whitespace resolves only to an exact key, such as the
    /// title `Manage Players`.
    pub fn resolve(&self, reference: &str) -> Option<String> {
        let r = reference.trim().trim_end_matches("()");
        if r.is_empty() {
            return None;
        }
        if r.contains(char::is_whitespace) {
            return self.index.get(r).cloned();
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
        let p = build(placed(&layout, files()), BuildOptions { fallback_title: Some("fallback"), ..BuildOptions::new(&layout, &meta) });
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
        assert_eq!(crosshair.modules[0].id, "PLUGIN", "plugin tables keep their name, the group has the title");
        assert_eq!(crosshair.modules[0].kind, ModuleKind::Handlers { object: "PLUGIN".to_string() });
        assert_eq!(crosshair.modules[0].functions[0].category, "Hooks");
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
        meta.insert("gm/plugins/stamina".to_string(), GroupMeta { name: Some("Stamina".into()), description: None, author: Some("kurozael".into()), version: Some("0.93".into()) });
        let files = vec![
            ("gm/gamemode/core/sh_kernel.lua".to_string(), scan("function Kernel.Init() end")),
            ("gm/plugins/stamina/plugin/sh_plugin.lua".to_string(), scan("PLUGIN:set_name('Ignored')\nPLUGIN:set_description('From code.')\nfunction PLUGIN:Think() end")),
            ("gm/plugins/sh_raisegun.lua".to_string(), scan("function PLUGIN:Think() end")),
            ("gm/other/x.lua".to_string(), scan("function x() end")),
        ];
        let p = build(placed(&layout, files), BuildOptions { title: Some("Catwork"), ..BuildOptions::new(&layout, &meta) });
        let sections: Vec<&str> = p.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(sections, vec!["Catwork", "Plugins"]);
        let core = &p.sections[0].groups[0];
        assert_eq!((core.title.as_str(), core.dir.as_str(), core.is_core), ("Catwork", "catwork", true));
        let plugins: Vec<(&str, &str)> = p.sections[1].groups.iter().map(|g| (g.title.as_str(), g.dir.as_str())).collect();
        assert_eq!(plugins, vec![("sh_raisegun", "plugins/sh_raisegun"), ("Stamina", "plugins/stamina")]);
        let stamina = &p.sections[1].groups[1];
        assert_eq!((stamina.description.as_deref(), stamina.author.as_deref(), stamina.version.as_deref()), (Some("From code."), Some("kurozael"), Some("0.93")));
        assert_eq!(stamina.modules[0].id, "PLUGIN");
        assert_eq!(p.resolve("Kernel.Init").as_deref(), Some("catwork/Kernel.html#Init"));
        assert_eq!(p.resolve("x"), None, "files outside every group are skipped");
    }

    #[test]
    fn never_merges_groups_of_different_specs() {
        let config = crate::layout::Config::from_yaml(
            "sections:
  - title: Plugins
    groups:
      - path: plugins/*
      - { path: extra/stamina, name: stamina }
      - path: gamemodes/a/plugins
      - path: gamemodes/b/plugins
",
        )
        .unwrap();
        let layout = config.layout.unwrap();
        let meta = HashMap::new();
        let files = vec![
            ("plugins/stamina/sh_plugin.lua".to_string(), scan("function Pattern.x() end")),
            ("extra/stamina/sh_plugin.lua".to_string(), scan("function Named.x() end")),
            ("gamemodes/a/plugins/sh_a.lua".to_string(), scan("function A.x() end")),
            ("gamemodes/b/plugins/sh_b.lua".to_string(), scan("function B.x() end")),
        ];
        let p = build(placed(&layout, files), BuildOptions { title: Some("X"), ..BuildOptions::new(&layout, &meta) });
        let groups: Vec<(&str, Vec<&str>)> = p.sections[0].groups.iter().map(|g| (g.dir.as_str(), g.modules.iter().map(|m| m.id.as_str()).collect())).collect();
        assert_eq!(groups, vec![("plugins/stamina", vec!["Named"]), ("plugins/plugins", vec!["A"]), ("plugins/plugins-2", vec!["B"]), ("plugins/stamina-2", vec!["Pattern"])]);
    }

    #[test]
    fn resolves_references() {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let p = build(placed(&layout, files()), BuildOptions { fallback_title: Some("fallback"), ..BuildOptions::new(&layout, &meta) });
        assert_eq!(p.resolve("Core::A#other").as_deref(), Some("flux/Core.A.html#other"));
        assert_eq!(p.resolve("Core.A:do_x").as_deref(), Some("flux/Core.A.html#do_x"));
        assert_eq!(p.resolve("Core.A").as_deref(), Some("flux/Core.A.html"));
        assert_eq!(p.resolve("g").as_deref(), Some("flux/Globals.html#g"));
        assert_eq!(p.resolve("player_meta#jump").as_deref(), Some("flux/Player.html#jump"));
        assert_eq!(p.resolve("Player:jump()").as_deref(), Some("flux/Player.html#jump"));
        assert_eq!(p.resolve("nothing"), None);
    }

    fn core(files: Vec<(&str, &str)>) -> Vec<Module> {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let files = files.into_iter().map(|(p, src)| (p.to_string(), scan(src))).collect();
        let p = build(placed(&layout, files), BuildOptions { title: Some("Flux"), ..BuildOptions::new(&layout, &meta) });
        p.sections.into_iter().next().map(|s| s.groups.into_iter().next().unwrap().modules).unwrap_or_default()
    }

    fn module<'a>(modules: &'a [Module], id: &str) -> &'a Module {
        modules.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("no module {id} in {:?}", modules.iter().map(|m| &m.id).collect::<Vec<_>>()))
    }

    #[test]
    fn assigns_realms_to_functions() {
        let modules = core(vec![
            ("lib/sv_a.lua", "function A.server() end\n--- Doc.\n-- @realm client\nfunction A.tagged() end"),
            ("lib/sh_b.lua", "function A.shared() end\nif CLIENT then\n  function A.client() end\nend"),
            ("lib/server/c.lua", "function A.dir() end"),
        ]);
        let realms: Vec<(&str, Realm)> = module(&modules, "A").functions.iter().map(|f| (f.name.as_str(), f.realm)).collect();
        assert_eq!(realms, vec![("client", Realm::Client), ("dir", Realm::Server), ("server", Realm::Server), ("shared", Realm::Shared), ("tagged", Realm::Client)]);
        let f = &module(&modules, "A").functions[0];
        assert_eq!(f.sources, vec![("lib/sh_b.lua".to_string(), 3, Realm::Client)]);
        assert_eq!(f.kind, FunctionKind::Declared);
    }

    #[test]
    fn merges_realm_twins() {
        let modules = core(vec![
            ("core/cl_kernel.lua", "local playerMeta = FindMetaTable('Player')\n--- Client data.\nfunction playerMeta:GetData(key) end\n--- Same.\nfunction cw.core:Same() end\nfunction cw.core:Undocumented() end\n--- Inc.\n-- @realm client\nfunction cw.core:IncludeSchema() end"),
            ("core/sv_kernel.lua", "local playerMeta = FindMetaTable('Player')\n--- Server data.\nfunction playerMeta:GetData(key, default) end\n--- Same.\nfunction cw.core:Same() end\n--- Documented.\nfunction cw.core:Undocumented() end\n--- Inc.\n-- @realm server\nfunction cw.core:IncludeSchema() end\nfunction cw.core:Twice() end\nfunction cw.core:Twice() end"),
            ("entities/entities/cw_item/shared.lua", "ENT.PrintName = 'Item'\n--- Shared.\nfunction ENT:GetItemTable() end\nfunction ENT:SetupDataTables() end"),
            ("entities/entities/cw_item/init.lua", "--- Server override.\nfunction ENT:GetItemTable() end\nfunction ENT:Think() end"),
            ("entities/entities/cw_item/cl_init.lua", "function ENT:Think() end\nfunction ENT:Draw() end"),
        ]);
        let get = |m: &str, name: &str| -> Vec<Function> { module(&modules, m).functions.iter().filter(|f| f.name == name).cloned().collect() };

        let data = get("Player", "GetData");
        assert_eq!(data.len(), 1);
        let data = &data[0];
        assert_eq!(data.realm, Realm::Shared);
        assert_eq!((data.file.as_str(), data.line), ("core/sv_kernel.lua", 3), "the server definition is primary");
        assert_eq!(data.params, vec!["key", "default"]);
        assert_eq!(data.summary(), "Server data.");
        assert_eq!(data.client_doc.as_ref().unwrap().summary(), "Client data.");
        assert_eq!(data.sources, vec![("core/sv_kernel.lua".to_string(), 3, Realm::Server), ("core/cl_kernel.lua".to_string(), 3, Realm::Client)]);

        assert!(get("cw.core", "Same")[0].client_doc.is_none(), "identical docs are kept once");
        assert!(get("cw.core", "IncludeSchema")[0].client_doc.is_none(), "docs differing only in @realm are identical");
        assert_eq!(get("cw.core", "IncludeSchema").len(), 1);
        let undocumented = &get("cw.core", "Undocumented")[0];
        assert_eq!(undocumented.summary(), "Documented.");
        assert!(undocumented.client_doc.is_none());
        let twice = get("cw.core", "Twice");
        assert_eq!(twice.len(), 2, "definitions in the same realm are not twins");
        let anchors: Vec<&str> = twice.iter().map(|f| f.anchor.as_str()).collect();
        assert_eq!(anchors, vec!["Twice", "Twice-2"]);

        let item = module(&modules, "cw_item");
        assert_eq!((item.title.as_str(), item.subtitle.as_deref()), ("Item", Some("Entity defined in entities/entities/cw_item")));
        assert_eq!(item.definition.as_deref(), Some("Entity"));
        assert_eq!(item.files.len(), 3);
        let table = &get("cw_item", "GetItemTable")[0];
        assert_eq!((table.realm, table.file.as_str(), table.summary().as_str()), (Realm::Shared, "entities/entities/cw_item/shared.lua", "Shared."));
        assert_eq!(table.sources.len(), 2);
        let think = &get("cw_item", "Think")[0];
        assert_eq!((think.realm, think.file.as_str(), think.sources.len()), (Realm::Shared, "entities/entities/cw_item/init.lua", 2));
        assert_eq!(get("cw_item", "Draw")[0].realm, Realm::Client);
    }

    #[test]
    fn keeps_the_client_doc_adopted_by_an_undocumented_twin() {
        let modules = core(vec![
            ("entities/entities/cw_lamp/cl_init.lua", "--- Draws the glow.\nfunction ENT:Think() end"),
            ("entities/entities/cw_lamp/init.lua", "function ENT:Think() end"),
            ("entities/entities/cw_lamp/shared.lua", "--- Updates the lamp.\nfunction ENT:Think() end"),
        ]);
        let think = &module(&modules, "cw_lamp").functions[0];
        assert_eq!((think.realm, think.file.as_str(), think.sources.len()), (Realm::Shared, "entities/entities/cw_lamp/shared.lua", 3));
        assert_eq!((think.summary(), think.doc_realm), ("Updates the lamp.".to_string(), Realm::Shared));
        assert_eq!(think.client_doc.as_ref().map(|d| d.summary()).as_deref(), Some("Draws the glow."), "the client doc survives the later merge");
    }

    #[test]
    fn merges_branch_twins() {
        let modules = core(vec![
            (
                "lib/sh_debug.lua",
                "local is_development = x
if is_development then
  --- Adds a metric.
  -- @param id [String]
  function add(id) end
  function Debug.both() end
else
  --- Does nothing.
  function add(id) end
  function Debug.both() end
end
if a then function twice() end end
if b then function twice() end end
if SERVER then function realms() end else function realms() end end
if c then function split() end else function Other.split() end end",
            ),
            ("lib/sv_x.lua", "if y then\n  --- Server y.\n  function X.y() end\nelse\n  --- Server stub.\n  function X.y() end\nend"),
            ("lib/cl_x.lua", "if y then\n  function X.y() end\nelse\n  function X.y() end\nend"),
        ]);
        let globals = module(&modules, "Globals");
        let get = |name: &str| -> Vec<&Function> { globals.functions.iter().filter(|f| f.name == name).collect() };

        let add = get("add");
        assert_eq!(add.len(), 1);
        let add = add[0];
        assert_eq!((add.summary().as_str(), add.line, add.realm), ("Adds a metric.", 5, Realm::Shared));
        let other = add.otherwise.as_ref().unwrap();
        assert_eq!((other.condition.as_str(), other.file.as_str(), other.line), ("is_development", "lib/sh_debug.lua", 9));
        assert_eq!(other.doc.as_ref().unwrap().summary(), "Does nothing.");
        assert_eq!(add.sources, vec![("lib/sh_debug.lua".to_string(), 5, Realm::Shared), ("lib/sh_debug.lua".to_string(), 9, Realm::Shared)]);

        let both = &module(&modules, "Debug").functions;
        assert_eq!(both.len(), 1);
        assert!(both[0].otherwise.as_ref().is_some_and(|o| o.doc.is_none()));

        assert_eq!(get("twice").len(), 2, "definitions in unrelated ifs stay apart");
        assert!(get("twice").iter().all(|f| f.otherwise.is_none() && f.sources.len() == 1));
        let realms = get("realms");
        assert_eq!(realms.len(), 1);
        assert!(realms[0].otherwise.is_none(), "server and client arms are realm twins");
        assert_eq!(realms[0].realm, Realm::Shared);
        assert!(get("split")[0].otherwise.is_none(), "different owners are not twins");

        let y = &module(&modules, "X").functions;
        assert_eq!(y.len(), 1, "branch twins of both realms merge into one");
        assert_eq!((y[0].realm, y[0].file.as_str(), y[0].line, y[0].sources.len()), (Realm::Shared, "lib/sv_x.lua", 3, 4));
        assert_eq!(y[0].summary(), "Server y.");
        let other = y[0].otherwise.as_ref().unwrap();
        assert_eq!((other.file.as_str(), other.line, other.doc.as_ref().map(|d| d.summary())), ("lib/sv_x.lua", 6, Some("Server stub.".to_string())));
    }

    #[test]
    fn builds_objects_from_local_tables() {
        let panels: String = (1..=3).map(|i| format!("local PANEL = {{}}\nfunction PANEL:Init() end\nvgui.Register('cw.panel{i}', PANEL, 'DPanel')\n")).collect();
        let modules = core(vec![
            ("core/libraries/sh_currency.lua", "--- The currency library.\nlibrary.New('currency', cw)\nlocal stored = cw.currency.stored or {}\nlocal CLASS_TABLE = { __index = CLASS_TABLE }\nfunction CLASS_TABLE:Query() end\nfunction stored.thing() end\nfunction cw.currency:Get() end"),
            ("core/libraries/sh_bars.lua", "library.New('bars', cw)\n--- A bar.\nlocal CLASS_TABLE = { __index = CLASS_TABLE }\nfunction CLASS_TABLE:Draw() end"),
            ("core/libraries/sh_loose.lua", "--- Loose objects.\nlocal CLASS_TABLE = {}\nfunction CLASS_TABLE:Draw() end"),
            ("core/derma/cl_character.lua", &panels),
            ("core/commands/sh_a.lua", "local COMMAND = cw.command:New('A')\nfunction COMMAND:OnRun() end"),
            ("core/blueprints/sh_chair.lua", "local BLUEPRINT = cw.blueprints:New()\nBLUEPRINT.name = 'Chair'\nfunction BLUEPRINT:OnBuild() end"),
            ("core/named.lua", "--- @module [Named]\nlocal T = setmetatable({}, mt)\nfunction T:x() end"),
            ("lib/sv_hooks.lua", "local PLUGIN = PLUGIN\nfunction PLUGIN:PlayerSpawn() end"),
            ("lib/cable.lua", "local cable = {}\nfunction cable.connect() end"),
            ("lib/sh_binds.lua", "local hooks = {}\nfunction hooks.Think() hook.Run('BindPressed') end"),
            ("core/alias.lua", "local util = util\nfunction util.thing() end\nlocal n = 1\nlocal mat = Material('x')\nfunction mat.y() end"),
        ]);
        let class = module(&modules, "cw.currency.Class");
        assert_eq!(class.kind, ModuleKind::Object { object: "CLASS_TABLE".to_string() });
        assert_eq!(class.subtitle.as_deref(), Some("Object table CLASS_TABLE in core/libraries/sh_currency.lua"));
        assert_eq!(class.decl_file, Some(("core/libraries/sh_currency.lua".to_string(), 4)));
        assert!(class.doc.is_none(), "the file doc belongs to the library");
        assert_eq!(module(&modules, "cw.currency").summary(), "The currency library.");
        assert_eq!(module(&modules, "stored").functions[0].name, "thing", "`x or {{}}` is not an object");
        assert_eq!(module(&modules, "cw.bars.Class").summary(), "A bar.");
        assert_eq!(module(&modules, "loose.Class").summary(), "Loose objects.", "the only object of a file without a library gets the file doc");
        for i in 1..=3 {
            assert_eq!(module(&modules, &format!("cw.panel{i}")).kind, ModuleKind::Object { object: "PANEL".to_string() });
        }
        assert_eq!(module(&modules, "A").functions[0].name, "OnRun");
        let chair = module(&modules, "chair.Blueprint");
        assert_eq!(chair.title, "Chair");
        assert_eq!(module(&modules, "Named").functions[0].name, "x");
        assert_eq!(module(&modules, "util").kind, ModuleKind::Library, "an alias is not an object");
        assert_eq!(module(&modules, "mat").kind, ModuleKind::Library, "a plain call is not an object");
        assert_eq!(module(&modules, "cable").kind, ModuleKind::Object { object: "cable".to_string() }, "other locals keep their name");
        assert_eq!(module(&modules, "hooks-binds").title, "hooks (binds)", "the hooks page keeps its id");
        assert_eq!(module(&modules, "Hooks").kind, ModuleKind::Hooks);
        assert_eq!(module(&modules, "PLUGIN").kind, ModuleKind::Handlers { object: "PLUGIN".to_string() });
    }

    #[test]
    fn recognises_object_initialisers() {
        for yes in ["{}", "{ __index = CLASS_TABLE }", "cw.command:New('A')", "item.New('x', true)", "setmetatable({}, mt)", "cw.theme:Begin()", "cw.x:New('aaaa…"] {
            assert!(is_object_init(yes), "{yes}");
        }
        for no in ["PLUGIN", "cw.currency.stored or {}", "1", "'str'", "Material('x')", "cw.x:New() or {}", "a.b", "item.get('x')", "x:new 'a'"] {
            assert!(!is_object_init(no), "{no}");
        }
        assert_eq!(constructor_name("cw.command:New('CharFallOver')").as_deref(), Some("CharFallOver"));
        assert_eq!(constructor_name("cw.system:New('Manage \\'Players\\'')").as_deref(), Some("Manage 'Players'"));
        assert_eq!(constructor_name("cw.attribute:New()"), None);
        assert_eq!(constructor_name("faction.New('#Faction_Admin')"), None, "language keys are not names");
        assert_eq!(object_word("CLASS_TABLE"), "Class");
        assert_eq!(object_word("PANEL"), "Panel");
        assert_eq!(object_word("ITEM_META"), "ItemMeta");
        assert_eq!(object_word("stored"), "Stored");
    }

    #[test]
    fn names_per_file_objects_and_entity_folders() {
        let modules = core(vec![
            ("entities/weapons/cw_hands/shared.lua", "SWEP.PrintName = 'Hands'\nfunction SWEP:PrimaryAttack() end"),
            ("entities/weapons/cw_baton/shared.lua", "SWEP.PrintName = 'Stun\\n  baton'\nfunction SWEP:PrimaryAttack() end"),
            ("entities/effects/blood/init.lua", "function EFFECT:Init() end"),
            ("items/a/sh_box.lua", "ITEM.name = 'Box'\nITEM.PrintName = '#Item_Box'\nfunction ITEM:OnUse() end"),
            ("items/b/sh_box.lua", "function ITEM:OnUse() end"),
            ("items/c/box.lua", "function ITEM:OnUse() end"),
        ]);
        let hands = module(&modules, "cw_hands");
        assert_eq!((hands.title.as_str(), hands.subtitle.as_deref(), hands.definition.as_deref()), ("Hands", Some("Weapon defined in entities/weapons/cw_hands"), Some("Weapon")));
        assert_eq!(module(&modules, "blood").subtitle.as_deref(), Some("Effect defined in entities/effects/blood"));
        assert_eq!(module(&modules, "cw_baton").title, "Stun baton");
        let boxes: Vec<(&str, &str)> = modules.iter().filter(|m| m.id.contains("box")).map(|m| (m.id.as_str(), m.title.as_str())).collect();
        assert_eq!(boxes, vec![("box", "box"), ("sh_box", "Box"), ("sh_box-b", "sh_box (b)")]);
        assert_eq!(module(&modules, "sh_box").subtitle.as_deref(), Some("Item defined in items/a/sh_box.lua"));
        assert_eq!(module(&modules, "sh_box").definition.as_deref(), Some("Item"));
        assert_eq!(module(&modules, "sh_box").badge(), "item");
    }

    #[test]
    fn builds_hook_pages() {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let files = vec![
            ("lib/sv_player.lua".to_string(), scan("local playerMeta = FindMetaTable('Player')\nfunction playerMeta:SetModel(m)\n  hook.Run('PlayerModelChanged', self, m)\nend")),
            ("lib/cl_player.lua".to_string(), scan("local playerMeta = FindMetaTable('Player')\nfunction playerMeta:SetModel(m)\n  --- Called when a model changes.\n  -- @param player [Player]\n  hook.Run('PlayerModelChanged', self, m, old)\nend\nlocal function helper()\n  hook.Run('PlayerModelChanged', x)\nend\nfunction cw.core:Init()\n  hook.Run('PlayerModelChanged')\n  if SERVER then hook.Run('ServerOnly') end\nend\nfunction GM:Initialize() end\nfunction GM:helper() end")),
            ("plugins/stamina/sh_plugin.lua".to_string(), scan("PLUGIN:SetGlobalAlias('cwStamina')\nfunction cwStamina:PlayerModelChanged(player) end\nfunction cwStamina:get_stamina() end\nfunction PLUGIN:Think() end\n--- Adds.\nhook.Add('ServerOnly', 'cwStamina.x', function(a, b) end)")),
            ("plugins/stamina/cl_hooks.lua".to_string(), scan("hook.Add('Think', 'x', function() end)")),
        ];
        let p = build(placed(&layout, files), BuildOptions { title: Some("Flux"), ..BuildOptions::new(&layout, &meta) });
        let modules = &p.sections[0].groups[0].modules;
        let ids: Vec<&str> = modules.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["Hooks", "GM", "cw.core", "Player"]);
        let hooks = &modules[0];
        assert_eq!((hooks.kind.clone(), hooks.subtitle.as_deref()), (ModuleKind::Hooks, Some("Hooks called by Flux")));
        let changed = &hooks.functions[0];
        assert_eq!(changed.name, "PlayerModelChanged");
        assert_eq!(changed.kind, FunctionKind::Hook);
        assert_eq!(changed.params, vec!["self", "m", "old"]);
        assert_eq!(changed.realm, Realm::Shared);
        assert_eq!(changed.summary(), "Called when a model changes.");
        assert_eq!((changed.file.as_str(), changed.line), ("lib/cl_player.lua", 5));
        assert_eq!(changed.sources.len(), 4);
        assert_eq!(changed.callers, vec!["Player:SetModel", "helper", "cw.core:Init"]);
        let server_only = &hooks.functions[1];
        assert_eq!((server_only.name.as_str(), server_only.realm), ("ServerOnly", Realm::Server));
        assert!(server_only.params.is_empty());

        let gm = &modules[1];
        assert_eq!(gm.kind, ModuleKind::Handlers { object: "GM".to_string() });
        assert_eq!(gm.kind.badge(), "gamemode");
        let categories: Vec<(&str, &str)> = gm.functions.iter().map(|f| (f.name.as_str(), f.category.as_str())).collect();
        assert_eq!(categories, vec![("helper", "Methods"), ("Initialize", "Hooks")]);

        let stamina = &p.sections[1].groups[0].modules;
        let ids: Vec<&str> = stamina.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["cwStamina", "hook.Add"]);
        let plugin = &stamina[0];
        assert_eq!(plugin.kind.badge(), "plugin");
        let fns: Vec<(&str, &str, Option<&str>)> = plugin.functions.iter().map(|f| (f.name.as_str(), f.category.as_str(), f.implements.as_deref())).collect();
        assert_eq!(fns, vec![("get_stamina", "Methods", None), ("PlayerModelChanged", "Hooks", Some("hook:PlayerModelChanged")), ("ServerOnly", "Hooks", Some("hook:ServerOnly")), ("Think", "Hooks", None)]);
        let added = plugin.functions.iter().find(|f| f.kind == FunctionKind::HookAdd).unwrap();
        assert_eq!((added.hook_id.as_deref(), added.params.clone(), added.summary()), (Some("cwStamina.x"), vec!["a".to_string(), "b".to_string()], "Adds.".to_string()));
        let hook_add = &stamina[1];
        assert_eq!((hook_add.title.as_str(), hook_add.kind.badge(), hook_add.functions[0].realm), ("hook.Add", "handlers", Realm::Client));

        for key in ["hook:PlayerModelChanged", "Hooks#PlayerModelChanged", "Hooks:PlayerModelChanged", "Hooks.PlayerModelChanged"] {
            assert_eq!(p.resolve(key).as_deref(), Some("flux/Hooks.html#PlayerModelChanged"), "{key}");
        }
        assert_eq!(p.resolve("PlayerModelChanged"), None, "hook names are not global functions");
        assert_eq!(p.resolve("cwStamina:PlayerModelChanged").as_deref(), Some("plugins/stamina/cwStamina.html#PlayerModelChanged"));
        assert_eq!(p.resolve("hook.Add#Think").as_deref(), Some("plugins/stamina/hook.Add.html#Think"));
    }

    #[test]
    fn attaches_file_docs() {
        let modules = core(vec![
            ("lib/cl_a.lua", "--- From the client file.\n\nfunction A.x() end"),
            ("lib/sh_a.lua", "--- From the shared file.\n\nfunction A.y() end"),
            ("lib/sv_a.lua", "--- From the server file.\n\nfunction A.z() end"),
            ("lib/b.lua", "--- Names B.\n-- @module [B]\n\nfunction A.w() end\nfunction B.x() end"),
            ("lib/c.lua", "--- Spans two modules.\n\nfunction C.x() end\nfunction D.x() end"),
            ("lib/sh_stdlib.lua", "--- +-+-+-+\n-- More text.\n\nfunction E.x() end"),
            ("lib/f.lua", "--- The Flux bars.\nmod 'Flux::Bars'\nfunction Flux.Bars:x() end\nfunction G.x() end"),
        ]);
        assert_eq!(module(&modules, "A").summary(), "From the shared file.");
        assert_eq!(module(&modules, "B").summary(), "Names B.");
        assert!(module(&modules, "C").doc.is_none() && module(&modules, "D").doc.is_none());
        assert!(module(&modules, "E").doc.is_none(), "banners are not docs");
        assert_eq!(module(&modules, "Flux.Bars").summary(), "The Flux bars.");
    }

    fn project(files: Vec<(&str, &str)>) -> Project {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let files = files.into_iter().map(|(p, src)| (p.to_string(), scan(src))).collect();
        build(placed(&layout, files), BuildOptions { title: Some("Flux"), ..BuildOptions::new(&layout, &meta) })
    }

    #[test]
    fn names_spaced_objects_without_spaces() {
        let p = project(vec![
            ("core/system/cl_manage_players.lua", "local SYSTEM = cw.system:New('Manage Players')\nfunction SYSTEM:OnDisplay()\n  hook.Run('GetPlayerScoreboardOptions')\nend"),
            ("core/system/cl_manage_config.lua", "--- The config system.\n-- @module [manage  config]\n\nlocal SYSTEM = cw.system:New('Other')\nfunction SYSTEM:OnDisplay() end"),
        ]);
        let modules = &p.sections[0].groups[0].modules;
        let players = module(modules, "ManagePlayers");
        assert_eq!((players.title.as_str(), players.slug.as_str()), ("Manage Players", "ManagePlayers"));
        let config = module(modules, "ManageConfig");
        assert_eq!((config.title.as_str(), config.summary().as_str()), ("manage  config", "The config system."));
        assert_eq!(module(modules, "Hooks").functions[0].callers, vec!["ManagePlayers:OnDisplay"]);
        for key in ["ManagePlayers", "Manage Players", " Manage Players "] {
            assert_eq!(p.resolve(key).as_deref(), Some("flux/ManagePlayers.html"), "{key}");
        }
        for key in ["ManagePlayers:OnDisplay", "ManagePlayers#OnDisplay", "Manage Players:OnDisplay"] {
            assert_eq!(p.resolve(key).as_deref(), Some("flux/ManagePlayers.html#OnDisplay"), "{key}");
        }
        assert_eq!(p.resolve("Manage  Players"), None, "a spaced reference must match a key exactly");
        assert_eq!(p.resolve("Manage Players:Nothing"), None);
        assert_eq!(name_id("give  item card"), "GiveItemCard");
        assert_eq!(name_id("cw.panel"), "cw.panel");
    }

    #[test]
    fn realm_file_docs_document_only_their_own_modules() {
        let modules = core(vec![
            ("lib/cl_h.lua", "--- Client half of H.\n\nfunction H.x() end"),
            ("lib/sv_h.lua", "function H.y() end"),
            ("lib/cl_k.lua", "--- Only on the client.\n\nfunction K.x() end"),
            ("lib/cl_m.lua", "--- Tagged M.\n-- @module [M]\n\nfunction M.x() end"),
            ("lib/sv_m.lua", "function M.y() end"),
            ("lib/n.lua", "--- Plain N.\n\nfunction N.x() end"),
            ("lib/sh_n.lua", "--- Shared N.\n\nfunction N.y() end"),
            ("entities/entities/cw_cash/cl_init.lua", "--- Client side of cw_cash.\n\nfunction ENT:Draw() end"),
            ("entities/entities/cw_cash/shared.lua", "--- Shared definition of cw_cash.\n\nfunction ENT:SetupDataTables() end"),
            ("entities/entities/cw_cash/init.lua", "function ENT:Use() end"),
        ]);
        assert!(module(&modules, "H").doc.is_none(), "a client file doc does not document a module the server file adds to");
        assert_eq!(module(&modules, "K").summary(), "Only on the client.", "every function of K is in the file");
        assert_eq!(module(&modules, "M").summary(), "Tagged M.", "@module overrides the realm rule");
        assert_eq!(module(&modules, "N").summary(), "Shared N.", "sh_ files still win");
        assert_eq!(module(&modules, "cw_cash").summary(), "Shared definition of cw_cash.");

        let p = project(vec![
            ("plugins/storage/plugin/cl_hooks.lua", "--- Client-side hooks of the Storage plugin.\n\nfunction PLUGIN:HUDPaint() end"),
            ("plugins/storage/plugin/sv_hooks.lua", "function PLUGIN:Think() end"),
        ]);
        let storage = &p.sections[0].groups[0].modules;
        assert_eq!(module(storage, "PLUGIN").functions.len(), 2);
        assert!(module(storage, "PLUGIN").doc.is_none(), "the handlers span both realms");
    }

    #[test]
    fn documents_hooks_from_gamemode_and_schema_handlers() {
        let modules = core(vec![
            ("lib/sh_player.lua", "function A.x()\n  hook.Run('PlayerModelChanged')\n  hook.Run('SchemaOnly')\n  hook.Run('PluginOnly')\n  --- From the call site.\n  hook.Run('Documented')\n  hook.Run('Undocumented')\nend"),
            ("gamemode/sh_hooks.lua", "--- From the gamemode.\nfunction GM:PlayerModelChanged() end\n--- From the gamemode too.\nfunction GM:Documented() end\nfunction GM:SchemaOnly() end\nfunction GM:Undocumented() end"),
            ("schema/sh_schema.lua", "--- From the schema.\nfunction Schema:PlayerModelChanged() end\n--- Only the schema.\nfunction Schema:SchemaOnly() end"),
            ("plugins/sh_x.lua", "--- What the plugin does.\nfunction PLUGIN:PluginOnly() end"),
        ]);
        let hooks = module(&modules, "Hooks");
        let docs: Vec<(&str, String, usize)> = hooks.functions.iter().map(|f| (f.name.as_str(), f.summary(), f.line)).collect();
        assert_eq!(
            docs,
            vec![
                ("Documented", "From the call site.".to_string(), 6),
                ("PlayerModelChanged", "From the gamemode.".to_string(), 2),
                ("PluginOnly", String::new(), 4),
                ("SchemaOnly", "Only the schema.".to_string(), 3),
                ("Undocumented", String::new(), 7),
            ]
        );
        assert!(hooks.functions.iter().all(|f| f.file == "lib/sh_player.lua"), "the location stays at the call site");
    }

    #[test]
    fn drops_callers_on_locals() {
        let modules = core(vec![(
            "lib/cl_menu.lua",
            "local panel = {}\nlocal function build()\n  local label = vgui.Create('x')\n  function label.DoClick()\n    hook.Run('Clicked')\n  end\n  function self.icon.DoClick()\n    hook.Run('Clicked')\n  end\n  function cw.menu.Late()\n    hook.Run('Clicked')\n  end\n  hook.Run('Clicked')\nend\nfunction panel.Open()\n  hook.Run('Clicked')\nend\nfunction Menu:Open()\n  hook.Run('Clicked')\nend",
        )]);
        let clicked = &module(&modules, "Hooks").functions[0];
        assert_eq!(clicked.callers, vec!["cw.menu.Late", "build", "panel.Open", "Menu:Open"]);
        assert_eq!(clicked.sources.len(), 6, "dropped callers keep their call sites");
    }

    #[test]
    fn reads_plugin_fields() {
        let layout = Layout::default_for("Flux");
        let meta = HashMap::new();
        let files = vec![("plugins/cl_crosshair.lua".to_string(), scan("local PLUGIN = PLUGIN\nPLUGIN.name = 'Crosshair'\nPLUGIN.author = 'Mr. Meow'\nPLUGIN.description = 'Adds a crosshair.'\nfunction PLUGIN:HUDPaint() end\nhook.Add('Think', 'x', function() end)"))];
        let p = build(placed(&layout, files), BuildOptions { title: Some("Flux"), ..BuildOptions::new(&layout, &meta) });
        let group = &p.sections[0].groups[0];
        assert_eq!((group.title.as_str(), group.author.as_deref(), group.description.as_deref()), ("Crosshair", Some("Mr. Meow"), Some("Adds a crosshair.")));
        let ids: Vec<&str> = group.modules.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["PLUGIN"], "hook.Add in a plugin file goes to the plugin");
        assert_eq!(group.modules[0].functions[0].realm, Realm::Client);
    }

    #[test]
    fn derives_definition_labels() {
        assert_eq!(local_definition("PANEL", "{}").as_deref(), Some("Panel"));
        assert_eq!(local_definition("COMMAND", "cw.command:New('A')").as_deref(), Some("Command"));
        assert_eq!(local_definition("CLASS", "Clockwork.class:New('Citizen')").as_deref(), Some("Class"));
        assert_eq!(local_definition("FACTION", "Clockwork.faction:New('Combine')").as_deref(), Some("Faction"));
        assert_eq!(local_definition("SYSTEM", "cw.system:New('Manage Players')").as_deref(), Some("System"));
        assert_eq!(local_definition("BLUEPRINT", "cw.blueprints:New()").as_deref(), Some("Blueprint"), "a plural library is named after the local");
        assert_eq!(local_definition("THING", "item.New('x', true)").as_deref(), Some("Item"));
        for (name, init) in [("CLASS_TABLE", "{}"), ("CLASS_TABLE", "{ __index = CLASS_TABLE }"), ("T", "setmetatable({}, mt)"), ("cable", "{}"), ("obj", "cw.thing:New()"), ("MAT", "cw.core:GetMaterial('x')"), ("X", "Cw.Thing:New()"), ("X", "make()")] {
            assert_eq!(local_definition(name, init), None, "{name} = {init}");
        }
        assert_eq!((plural("Entity"), plural("Class"), plural("Command"), plural("Key")), ("Entities".to_string(), "Classes".to_string(), "Commands".to_string(), "Keys".to_string()));

        let modules = core(vec![
            ("core/commands/sh_a.lua", "local COMMAND = cw.command:New('A')\nfunction COMMAND:OnRun() end"),
            ("core/derma/cl_menu.lua", "local PANEL = {}\nfunction PANEL:Init() end\nvgui.Register('cw.menu', PANEL)"),
            ("core/libraries/sh_bars.lua", "library.New('bars', cw)\nlocal CLASS_TABLE = {}\nfunction CLASS_TABLE:Draw() end"),
            ("core/named.lua", "local T = setmetatable({}, mt)\nfunction T:x() end"),
            ("commands/sh_addbots.lua", "CMD.name = 'AddBots'\nfunction CMD:on_run() end"),
            ("entities/entities/fl_money/shared.lua", "ENT.PrintName = 'Money'\nfunction ENT:Use() end"),
        ]);
        let command = module(&modules, "A");
        assert_eq!((command.definition.as_deref(), command.subtitle.as_deref(), command.badge()), (Some("Command"), Some("Command defined in core/commands/sh_a.lua"), "command".to_string()));
        assert_eq!(module(&modules, "cw.menu").definition.as_deref(), Some("Panel"));
        let class = module(&modules, "cw.bars.Class");
        assert_eq!((class.definition.as_deref(), class.badge()), (None, "object".to_string()));
        assert_eq!(module(&modules, "named.T").definition, None);
        let cmd = module(&modules, "sh_addbots");
        assert_eq!((cmd.title.as_str(), cmd.definition.as_deref(), cmd.subtitle.as_deref()), ("AddBots", Some("Command"), Some("Command defined in commands/sh_addbots.lua")));
        let money = module(&modules, "fl_money");
        assert_eq!((money.definition.as_deref(), money.subtitle.as_deref()), (Some("Entity"), Some("Entity defined in entities/entities/fl_money")));
    }

    #[test]
    fn lists_definitions_by_kind() {
        let p = project(vec![
            ("lib/sh_a.lua", "function A.x() end"),
            ("lib/sh_b.lua", "local T = setmetatable({}, mt)\nfunction T:x() end"),
            ("entities/entities/fl_money/shared.lua", "function ENT:Use() end"),
            ("commands/sh_b.lua", "CMD.name = 'beta'\nfunction CMD:on_run() end"),
            ("commands/sh_a.lua", "CMD.name = 'Alpha'\nfunction CMD:on_run() end"),
            ("items/sh_box.lua", "function ITEM:on_use() end"),
            ("views/cl_menu.lua", "function PANEL:Init() end"),
            ("blueprints/sh_chair.lua", "local BLUEPRINT = cw.blueprints:New()\nfunction BLUEPRINT:OnBuild() end"),
            ("systems/sh_x.lua", "local SYSTEM = cw.system:New('X')\nfunction SYSTEM:OnDisplay() end"),
        ]);
        let group = &p.sections[0].groups[0];
        let code: Vec<&str> = group.code_modules().map(|m| m.id.as_str()).collect();
        assert_eq!(code, vec!["A", "b.T"]);
        let kinds = group.definitions();
        let definitions: Vec<(&str, Vec<&str>)> = kinds.iter().map(|(label, ms)| (label.as_str(), ms.iter().map(|m| m.title.as_str()).collect())).collect();
        assert_eq!(definitions, vec![
            ("Commands", vec!["Alpha", "beta"]),
            ("Items", vec!["sh_box"]),
            ("Entities", vec!["fl_money"]),
            ("Panels", vec!["cl_menu"]),
            ("Blueprints", vec!["chair.Blueprint"]),
            ("Systems", vec!["X"]),
        ]);
    }
}
