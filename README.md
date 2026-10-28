# fdoc

`fdoc` reads the documentation comments of a Lua code base, along with its
[NL](https://github.com/nl-lang/nl) files, and generates a static HTML
reference: one page per class, library, hook table, template object, Lumen
template, NL struct, enum, interface or module, with a sidebar, full-text
search, cross-reference links, syntax highlighted examples and a night mode.
The output is plain files and works from disk or any static host. The same
model can be written as one JSON document instead, for tools to read (see
[JSON output](#json-output)).

## Usage

```sh
cargo build --release
./target/release/fdoc path/to/source -o path/to/docs
```

It reads every `.lua` file, every Lumen template (`.lumen`, see
[below](#lumen-templates)) and every NL file (`.nl` and `.nlua`, see
[below](#nl-files)) below the source directory. NL declaration files
(`.d.nl`) are not read. Lua files may use the syntax of Garry's Mod (`!`,
`!=`, `&&`, `||`, `//` and `/* */` comments, `continue`) and the LuaJIT 3.0
extensions: bit operators (`&`, `|`, `~`, `<<`, `>>`, `~>>`), `c ? a : b`,
`a?.b`, `a?.[k]`, `a?.(...)`, `a?.:m()`, `a ?? b`, compound assignments
(`+=`, `..=`, `~>>=`, ...), `const`, short functions (`x -> e`,
`|a, b| -> e`, `|| -> e`, `|a| -> do ... end`), `1_000`, `\u{XXXX}` escapes
and `;;`.

| Option | Effect |
| --- | --- |
| `-o, --output <DIR>` | Output directory (default: `<SOURCE_DIR>/docs`); `-` writes the JSON document to standard output |
| `-f, --format <FORMAT>` | `html` (default) writes the pages, `json` the whole model as `<DIR>/fdoc.json` |
| `--config <FILE>` | Layout and project settings (default: `<SOURCE_DIR>/.fdoc.yml` if it exists); see [below](#configuration) |
| `--title <NAME>` | Project name (default: the config's `title`, else the `name` in `packagespec.lua`, else the directory name) |
| `--source-url <URL>` | Base URL that source paths are appended to for "view source" links |
| `--exclude <NAME>` | Directory name to skip anywhere, or a path with a `/` (such as `./lib` or `a/b.lua`) relative to the source directory; repeatable (default: `docs`, `.git`) |
| `--documented-only` | Only include functions that have a doc comment |
| `--clean` | Delete the output directory before generating |
| `-q, --quiet` | Only print errors |

Example for Flux:

```sh
fdoc ~/code/flux-ce -o ~/code/flux-ce/docs \
  --source-url https://github.com/TeslaCloud/flux-ce/blob/master/
```

With `--format json`, the summary goes to standard error when the document
goes to standard output:

```sh
fdoc ~/code/flux-ce -f json -o - | jq '.sections[].groups[].modules[].id'
```

The pages are static, so they can be opened straight from disk. When they are
served over HTTP, following a link fetches only the content column and keeps the
sidebar in place, which makes moving between modules feel instant. Any static
server works, for example:

```sh
python3 -m http.server --directory path/to/docs
```

## JSON output

`--format json` writes the model the pages are made from as one JSON
document, `fdoc.json`. It is written in ASCII: every other character is
escaped as `\uXXXX`. Its fields are listed below; a field marked `?` is
`null` when there is nothing to put in it. `format_version` is raised when
a field is renamed, removed or changes its meaning; new fields can be added
without it.

```text
{ title, version?, summary?, description?, source_url?,
  format_version: 1, generator: "fdoc <version>",
  sections: [Section],
  index: { <reference>: <page URL> } }        # how references such as `A#b` resolve

Section  { title, groups: [Group] }

Group    { title, path,                       # the output directory of its pages
           source_dir,                        # its directory in the source tree
           core: bool, description?, author?, version?,
           realm?,                            # "server" for a server-side package
           modules: [Module] }

Module   { id, title, subtitle?,
           kind,                              # the badge: "class", "library", "hooks", "gamemode",
                                              # "schema", "plugin", "handlers", "globals", "object",
                                              # "template", "module", "struct", ... or a definition
                                              # label in lower case ("command", "entity")
           module_kind,                       # "class", "library", "hooks", "handlers", "globals",
                                              # "object" or "template"
           keyword?,                          # an NL class's keyword ("struct", "enum", ...)
           extends?,                          # the class it extends
           object?,                           # handlers and objects: the table as written ("GM", "PANEL")
           definition?,                       # a definition's label ("Command", "Template")
           realm?,                            # a Lumen template's realm
           summary, description, doc: Doc?,
           files: [path], declared_in: { file, line }?, slug,
           categories: [{ name, description }],
           functions: [Function] }

Function { name, owner, qualified,            # "Owner:name", "Owner.name" or "name"
           signature,                         # qualified, with the parameters ("A:b(x, y)")
           params: [name],                    # for a hook: its arguments, as written
           kind,                              # "declared", "hook", "hook_add" or "value"
           realm,                             # "shared", "server" or "client"
           file, line,                        # the primary definition
           sources: [{ file, line, realm }],  # every definition; a hook's call sites
           summary, description, doc: Doc?,
           doc_realm,                         # the realm of the definition `doc` comes from
           realm_docs: [{ realm, summary, description, doc: Doc }],
                                              # merged twins: the docs of the other realms
           otherwise: { condition, file, line, summary, description, doc: Doc? }?,
                                              # branch twins: the definition in the other arm
           callers: [qualified],              # hooks: the functions that run it
           hook_id?,                          # hook.Add handlers: the identifier
           implements?,                       # hook handlers: the hook's reference ("hook:Name")
           category, part?,
           ty?, init?,                        # NL: the type, and what a value is given
           anchor }                           # the fragment of its entry on the module's page

Doc      { params: [{ name, type?, default?, description }],
           returns: [{ type?, description }],
           variants: [{ signature, description, params, returns }],
           see: [reference], alias: [name],
           deprecated?,                       # the reason ("" without one) when deprecated
           deprecation_version?,
           warning?,                          # the first warning's text
           warnings: [{ label?, text }], internal: bool,
           environment?, environment_text?,   # @environment's name and text
           realm?,                            # @realm
           category?, module?, ignored: bool,
           examples: [{ language?, code }],   # the fenced code blocks of the description
           tags: [{ name, text }] }           # tags fdoc does not know
```

Hooks are the functions of the `Hooks` modules (`module_kind: "hooks"`),
with `kind: "hook"`, their call sites in `sources` and their callers in
`callers`.

## Layout of the generated documentation

The documentation is split into sections, shown as tabs in the sidebar, and
each section into groups of source files. A core group has no index page of
its own: its modules are listed directly under the section. Every other group
(a package or a plugin) gets an index page with its description, author and
version.

Without a configuration file the layout follows Flux:

- the project title is the section of the core, which holds every file not
  claimed by a package or a plugin;
- `packages/<name>/...` becomes a group in the "Packages" section, named after
  its `packagespec.lua`;
- `plugins/<name>/...` and `plugins/<file>.lua` become groups in the "Plugins"
  section, named through `PLUGIN:set_name`.

Sections without groups are left out.

### Configuration

Other layouts are described in a `.fdoc.yml` file in the source directory, or
the file given with `--config`. All keys are optional; command line options
take precedence over the file. For example, for Catwork, a
Clockwork-derived gamemode:

```yaml
title: Catwork                 # same as --title
version: "0.95"
summary: One-line project summary shown on the index page.
description: |
  Longer Markdown description for the index page.
source_url: https://github.com/Meow/Catwork/blob/master/   # same as --source-url
exclude:                       # added to --exclude
  - thirdparty
documented_only: false         # same as --documented-only
sections:                      # sidebar tabs, in order
  - title: Catwork
    groups:
      - path: gamemodes/catwork/gamemode   # one group from this directory
        name: Catwork                      # fixed title
        core: true                         # modules listed directly under the section
  - title: Plugins
    groups:
      - path: gamemodes/catwork/plugins/*      # one group per directory
      - path: gamemodes/catwork/plugins/*.lua  # and one per file
  - title: HL2RP
    groups:
      - path: gamemodes/cwhl2rp/schema
        name: HL2RP
        core: true
  - title: HL2RP Plugins
    groups:
      - path: gamemodes/cwhl2rp/plugins/*
```

An `exclude` entry without a `/`, or with only a trailing one (`thirdparty/`),
skips every directory of that name. One with any other `/`, such as `./lib`,
`gamemodes/catwork/gamemode/thirdparty` or `./packagespec.lua`, skips the
directory or file at that path, relative to the source directory.

A group `path` is relative to the source directory. A `*` component matches
any directory and makes one group per match; `*.lua` as the last component
matches any Lua or NL file and makes one group per file. `.` is the whole
source tree. When a file matches several paths, the one with the most
components that are not wildcards wins, and `.` loses to all others. Files
that match no path are left out of the documentation, and their number is
reported. A group directory and a file of the same name in one section, such
as `plugins/stamina/` and `plugins/stamina.lua`, form one group.

A group's title is the first of:

1. its `name` in the configuration;
2. the `name` in a Flux-style `plugin.yml` or a Clockwork-style `plugin.ini`
   in the group's directory (`plugins/stamina/plugin.yml` for `plugins/*`),
   which also supplies the description, the author and the version (for
   `plugin.ini`: `version`, else `compatibility`);
3. the `name` in a `packagespec.lua` in the group's directory, which also
   supplies the summary, the author and the version;
4. the name given to `PLUGIN:set_name`, along with `PLUGIN:set_description`
   and `PLUGIN:set_author`;
5. the directory or file name that `*` matched.

A core group is titled by its `name`, `plugin.yml` or `plugin.ini`, else after
its section.

Pages are written to `<section>/` for the first core group of a section and to
`<section>/<group>/` for the others, where `<section>` is the lower-cased
section title and `<group>` the group's `name`, its last path component, or
what `*` matched. When a section has several `*` paths (a `*` path and the
`*.lua` path next to it count as one), the groups of each go to
`<section>/<path>/<group>/` instead, where `<path>` is the path's components
other than `*` joined with `-`, such as `plugins/gamemodes-a-plugins/stamina/`.
Groups of different paths are never merged: a directory that is taken already
gets a `-2`, `-3`, ... suffix. The default layout writes the core
to `core/`, packages to `packages/<name>/` and plugins to `plugins/<name>/`.

### Modules

Inside each group, functions are collected into modules by the table they are
defined on: `function Foo.Bar:baz()` lands in `Foo.Bar`. Metatable locals such
as `local player_meta = FindMetaTable('Player')` are shown as `Player`, and
free functions are listed under `Globals`.

- **Hooks.** Every group that runs hooks (`hook.Run`, `hook.Call`,
  `Plugin.call`, `cw.plugin:Call`, ...) gets a `Hooks` page that defines
  them, with one entry per hook name: the arguments of its widest call,
  the functions that call it and every call site. The doc comment above a
  call documents the hook, else the doc comment of the `GM`, then the
  `Schema` handler of the same name in the group; without `@param` tags,
  the call's arguments are listed as the expressions they are. Callers
  that are functions on a local or a `self` field (`label.DoClick`) cannot
  be linked and are left out; their call sites are still listed.
- **Hook handlers.** `GM`, `Schema` and the plugin table (`PLUGIN`, shown under
  the name given to `PLUGIN:set_global` if any) get their own pages, badged
  `gamemode`, `schema` and `plugin`. Their PascalCase methods are listed as
  hooks, the others as methods. Handlers added with `hook.Add` join the page
  of the file's plugin or gamemode table, else a `hook.Add` page (badged
  `handlers`), and show their identifier. A handler of a hook the project
  runs links to that hook's entry on a `Hooks` page.
- **Objects.** A file-local table such as `local PANEL = {}`,
  `local CLASS_TABLE = { __index = CLASS_TABLE }` or
  `local COMMAND = cw.command:New('A')` gets a page of its own. It is named by
  `@module`, `vgui.Register`, the constructor's string argument, a
  `PrintName` / `name` field, or else after the file's library
  (`cw.currency.Class`). A name with spaces is the page's title, and its
  words are joined for the id (`cw.system:New('Manage Players')` is
  `ManagePlayers`); references can use either. Aliases such as `local PLUGIN = PLUGIN` are not
  objects. Template objects that the loader injects (`PANEL`, `TOOL`, `ITEM`,
  `ENT`, ...) get one page per file, and an entity, weapon or effect folder
  (`init.lua`, `cl_init.lua`, `shared.lua`) gets one page named after the
  folder. A local created by a call on a template object, such as a
  subcommand (`local add = CMD:subcommand('add')`), is part of that
  object's page rather than an object of its own: its functions are listed
  there under the call's string argument (`add:on_run`), in a category
  named after the method (`Subcommands`), and references name them through
  the page (`sh_ban.add:on_run`).
- **Definitions.** Objects that the framework loads from their own file,
  such as commands, items, factions, classes, roles, attributes, conditions,
  entities, weapons, effects, tools, panels, themes and skins, are
  definitions rather than code, and so are Lumen templates. Every template
  object but a `PACKAGE` installer is one, labelled after its name (`CMD` is
  a `Command`, `ENT` an `Entity`, `SWEP` a `Weapon`), and
  so is a file-local object named like one (`local PANEL = {}`,
  `local COMMAND = ...`, `local CLASS = ...`) or, failing that, an all-caps
  local created by a constructor call on a library, labelled after the
  library (`local SYSTEM = cw.system:New('A')` is a `System`). Other tables,
  such as `local CLASS_TABLE = {}`, and `setmetatable(...)` stay code. In the
  sidebar, on a group's index page and on the project index, a group lists
  its code modules first, then its definitions under a heading per kind
  (`Commands`, `Items`, `Entities`, ...). A definition is badged with its
  label (`command`, `entity`, `template`), and its page says where it is
  defined (`Command defined in commands/sh_kick.lua`).
- **Lumen templates.** Each `.lumen` file gets a page of its own, titled by
  its template ID (the file name without the extension) and listed under
  `Templates`. Its file doc comment describes it, and its documented
  top-level `local function`s, its components and helpers, are listed on it;
  references name them through the template (`help_commands#Command`).
  Functions it defines on tables, such as `function Foo.bar()`, land in their
  modules as usual.
- **Realms.** Each function is marked server (blue), client (orange) or
  shared (both), from `@realm`, then `if SERVER` / `if CLIENT` blocks, then
  the file name; Lumen templates run on the client. An NL file counts as
  the Lua file it stands for (`init.nl` is `init.lua`, `cl_hud.nl` is a
  client file). In a package whose
  `packagespec.lua` sets `s.serverside = true`, a file whose path does not
  make it client-side (`cl_` files, `client/` directories, templates)
  counts as a server file; the nearest `packagespec.lua` above a file
  decides, whatever the layout, and the package's index page is marked
  server-side. The marker is shown in the summary, the details, the sidebar
  and search results, and next to the title of a module that runs on one
  side only: a template, or a module whose functions all do. A function
  defined in more than one realm (such as in `sv_kernel.lua` and
  `cl_kernel.lua`, or in an entity's `shared.lua` and `init.lua`) is shown
  once, as shared, with every definition; when their doc comments differ,
  each is shown under its realm ("Shared", "On the server", "On the
  client").
- **Branch twins.** A function defined in the first arm of an `if`
  statement and again in one of its `elseif` / `else` arms, for the same
  realm, such as a real implementation under
  `if is_development then` and a stub that does nothing under `else`, is
  shown once with both definitions. The first arm's definition is the
  primary one; the other's doc follows under "Otherwise", introduced by the
  condition that does not hold (`When is_development does not hold:`), with
  its first paragraph and parameters left out when they repeat the primary
  doc's. When the primary doc has an `@environment` tag, the block is titled
  "In other environments" instead. Only the innermost `if` around a
  definition counts, so definitions in two separate `if` statements stay
  apart. Branch twins in a server and a client file are then merged as
  above.

A file's doc comment documents the module its `@module` names, else the
library it declares, else the only module its functions are in. Without
`@module`, a server or client file documents a module only when all of the
module's functions are in that file, so the client half of a module does
not describe the whole. `sh_` files win over the others.

Local functions are private and not documented, except the top-level ones
of a Lumen template. A local function is a `local function f()`, a
`const function f()`, or a local given a short function
(`local f = |a, b| -> ...`, `const f = x -> ...`), whose parameters are
those of the short function. In NL, top-level functions, constants and types without
`pub` are private.

### Lumen templates

A Lumen template is Lua code in which markup may stand wherever an
expression may, such as `return <view style={style}>Hi, {name}!</view>`.
Its markup is skipped, while the Lua code in its `{ }` splices (attribute
values, `{...spread}` and child expressions) is read like any other code,
so that hooks run from an event handler inside the markup are found. A `<`
starts markup as it does for the Lumen compiler: when a name or `>` follows
it and the token before it cannot end an operand, so `a < b` stays a
comparison. Templates are placed in groups like any other file, so the
templates of a package or a plugin (in its `views/lumen/` folder) belong to
it.

### NL files

NL files are read with NL's own parser. An NL file stands for the Lua file
of the same name, as it does on a server with NL's `lua_shared`: it is
placed in groups, named, given a realm and made part of an entity folder as
`x.lua` would be. Its doc comments are `##` lines (see
[Doc comments](#doc-comments)). What it declares is documented as follows:

- **Exports.** The `pub fn`, `pub const` and `pub type` declarations of a
  file are the members of a module badged `module` and named by the path
  NL imports the file by: the path below the last `lua` directory, as in an
  addon (`addons/inv/lua/inv/sh_items.nl` is `inv/sh_items`), else below
  `gamemodes`, else below the source directory. The file's doc comment
  documents it.
- **Types.** A `struct`, `enum` or `interface`, `pub` or not, gets a page
  badged with its keyword, listing its fields (`weight: Number = 1`),
  variants (`Back = 2`) or members first, then its methods
  (`fn Item.use(by: Player): Bool`), and its operators. The methods of an
  `interface Named for Item` implementation are listed on `Item`'s page in a
  category named `Named`. A struct's parent (`struct Crate : Item`) is
  shown as the class it extends.
- **Modules.** A `module Inventory ... end` gets a page badged `module`
  with its functions and constants; a nested one is `Inventory.Filters`.
  A module that is reopened, in the same or another file, is one page.
- **Globals.** A `global Name = value` is listed under `Globals`, as a
  function when the value is a function.
- **Lua paths.** `:Owner.name = fn(...) ... end` defines a function on a
  Lua table, as `Owner.name = function(...)` does in Lua: on `GM`,
  `Schema` or `PLUGIN` it is a hook handler, on `ENT`, `PANEL` and the
  other template objects it is part of the file's object. `:PLUGIN.name =
  'Admin'`, `:ENT.PrintName = 'Crate'` and `:PLUGIN:set_name('Admin')` set
  metadata as their Lua counterparts do.
- **Hooks.** `on PlayerSpawn [as 'id'] do |ply| ... end`, `Hook.add` and
  `Event.on_run` add hook handlers (NL's events are Garry's Mod's hooks);
  `Event.run`, `Hook.run`, `Hook.call` and Lua paths such as `:hook.Run`
  run hooks.
- **Realms.** `if SERVER`, `if not CLIENT`, `unless CLIENT`, their `elseif`
  and `else` arms, and statement modifiers such as `Event.run 'X' if SERVER`
  decide the realm, and the arms of one `if` make branch twins, as in Lua.

Signatures are shown as written, with their types and defaults
(`find(name: String, *rest): Item?`). The types and defaults of the
signature fill in what a doc comment leaves out: without `@param` tags,
every parameter is listed with its type; a `@param` without a type gets the
signature's; without `@return`, the return types are listed. In NL, a
`@param name text` without brackets is a description, and so is the first
word inside the brackets of a `@param` or `@return` when it does not look
like an NL type (`@return [the health left]`).

## Doc comments

A doc comment starts with `---` and continues on the following `--` lines. It
documents the declaration that directly follows it: a `function`, a
`name = function` assignment or one of a short function
(`Lib.add = |a, b| -> a + b`), a `class 'Name'` declaration, a top-level
`local NAME = ...` or `const NAME = ...` (such as `local PANEL = {}`), a
top-level local function of a Lumen template, or a statement that runs a
hook (`hook.Run('Name', ...)`) or adds one (`hook.Add('Name', ...)`).

In an NL file, a doc comment starts with `##` and continues on the following
`##` or `#` lines, as for NL's own tools. It documents the declaration or
statement that directly follows it: a `fn`, `struct`, `enum`, `interface`,
`module`, `type`, `const` or `global` declaration, a struct field, an enum
variant, an interface member, an `on` handler, a `:Owner.name = fn`
assignment or a statement that runs or adds a hook. Fenced code in it
without a language is highlighted as NL.

The first doc comment that is not attached to anything and comes before the
first declaration of the file documents the file itself; `@module` names the
module it describes.

The summary shown in listings is the first sentence of the description.

```lua
--- Summary line, followed by a longer description in Markdown.
--
-- Fenced code blocks are rendered as highlighted examples:
-- ```
-- test(123, player, { test = true })
-- ```
--
-- @param a=Default value [Number What `a` is for]
-- @param b [Object Some object]
-- @return [Foo blank foo object, Number one hundred]
-- @see other_function
-- @see [MyClass#method]
function test(a, b)
end
```

Supported tags:

| Tag | Meaning |
| --- | --- |
| `@param name[=default] [Type description]` | A parameter. The default marks it optional. Without brackets or a default, the text after the name is its description. |
| `@return [Type description, Type description]` | Return values; a comma followed by a type starts the next value. |
| `@variant name(a, b)` | An alternative signature. Indented `@param` and `@return` lines below it belong to the variant, and the text just above it describes it. A `@return` that is not indented applies to the whole function. |
| `@see [Reference]` or `@see Reference` | A related function or module (`Owner#name`, `Owner:name`, `Owner.name`, `name` or `Owner`). |
| `@warning [Label] text` | A warning box. The `Internal` label marks the function as internal. |
| `@environment [name] text` or `@env` | The code only does its work in the named environment (`development`, `production`, ...). The name is shown as a marker next to the function, and the doc starts with a box titled after it (`Development only`) that holds the text, else "Only available in the development environment.". In a file's doc comment, it marks the module the file documents, such as a file that returns early outside of development. |
| `@deprecation [reason]` | Marks the function as deprecated. |
| `@deprecation_version [version]` | The version the deprecation starts at. |
| `@alias [Other.name]` | Another name the function is available under. |
| `@category [Name]` | On its own, starts a named section that groups the functions below it in the same file. The text after it describes the section. |
| `@realm [server\|client\|shared]` | Where the code runs, overriding the realm derived from the file name (`sv_`, `cl_`, `sh_`, `init.lua`, `init.nl`, `.lumen`, ...), from a server-side package and from `if SERVER` / `if CLIENT` blocks. |
| `@module [Name]` | In the file's doc comment, the module the file documents (`@module [cw.currency]`); above a `local` table, the name of the object it defines. |
| `@ignore` | Skips the declaration. |

Bracketed tag bodies may continue on the next lines. Inline code that names a
documented function or module, such as `` `Player:add_item` ``, is turned into
a link.

### Types

The type is the first word inside the brackets of a `@param` or `@return`; the
rest is the description. Type names are not checked against anything and are
shown as written, so they are a convention rather than a syntax. They are
capitalised: `Number`, `String`, `Boolean`, `Function`, `Nil`.

A type may also be the name of a class, written as the class is named in the
code: `Player`, `Item` or `ActiveRecord.Base`.

Lua has a single table type, so tables are named after how they are used:

| Type | Meaning |
| --- | --- |
| `Map` | A key-value table, such as `{ name = 'crowbar', weight = 2 }`. |
| `List` | An integer-indexed table, such as `{ 'a', 'b', 'c' }`. |

The element type may be added in angle brackets, as in `List<Item>`. A type
cannot contain spaces, since the first space ends it.

## Development

```sh
cargo test
```

## License

Released under the [MIT License](LICENSE).
