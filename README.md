# fdoc

`fdoc` reads the documentation comments of a Lua code base and generates a
static HTML reference: one page per class, library, hook table or template
object, with a sidebar, full-text search, cross-reference links, syntax
highlighted examples and a night mode. The output is plain files and works
from disk or any static host.

## Usage

```sh
cargo build --release
./target/release/fdoc path/to/source -o path/to/docs
```

| Option | Effect |
| --- | --- |
| `-o, --output <DIR>` | Output directory (default: `<SOURCE_DIR>/docs`) |
| `--config <FILE>` | Layout and project settings (default: `<SOURCE_DIR>/.fdoc.yml` if it exists); see [below](#configuration) |
| `--title <NAME>` | Project name (default: the config's `title`, else the `name` in `packagespec.lua`, else the directory name) |
| `--source-url <URL>` | Base URL that source paths are appended to for "view source" links |
| `--exclude <NAME>` | Directory name, or path relative to the source directory, to skip; repeatable (default: `docs`, `.git`) |
| `--documented-only` | Only include functions that have a doc comment |
| `--clean` | Delete the output directory before generating |
| `-q, --quiet` | Only print errors |

Example for Flux:

```sh
fdoc ~/code/flux-ce -o ~/code/flux-ce/docs \
  --source-url https://github.com/TeslaCloud/flux-ce/blob/master/
```

The pages are static, so they can be opened straight from disk. When they are
served over HTTP, following a link fetches only the content column and keeps the
sidebar in place, which makes moving between modules feel instant. Any static
server works, for example:

```sh
python3 -m http.server --directory path/to/docs
```

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

An `exclude` entry without a `/` skips every directory of that name; one with
a `/` skips that path, relative to the source directory.

A group `path` is relative to the source directory. A `*` component matches
any directory and makes one group per match; `*.lua` as the last component
matches any file and makes one group per file. `.` is the whole source tree.
When a file matches several paths, the one with the most components that are
not wildcards wins, and `.` loses to all others. Files that match no path are
left out of the documentation, and their number is reported. A group directory
and a file of the same name in one section, such as `plugins/stamina/` and
`plugins/stamina.lua`, form one group.

A group's title is the first of:

1. its `name` in the configuration;
2. the `name` in a Clockwork-style `plugin.ini` in the group's directory
   (`plugins/stamina/plugin.ini` for `plugins/*`), which also supplies the
   description, the author and the version (`version`, else `compatibility`);
3. the `name` in a `packagespec.lua` in the group's directory, which also
   supplies the summary, the author and the version;
4. the name given to `PLUGIN:set_name`, along with `PLUGIN:set_description`
   and `PLUGIN:set_author`;
5. the directory or file name that `*` matched.

A core group is titled by its `name` or `plugin.ini`, else after its section.

Pages are written to `<section>/` for the first core group of a section and to
`<section>/<group>/` for the others, where `<section>` is the lower-cased
section title. The default layout keeps Flux's directories: `flux/`,
`packages/<name>/` and `plugins/<name>/`.

### Modules

Inside each group, functions are collected into modules by the table they are
defined on: `function Foo.Bar:baz()` lands in `Foo.Bar`. Metatable locals such
as `local player_meta = FindMetaTable('Player')` are shown as `Player`, and
free functions are listed under `Globals`.

- **Hooks.** Every group that runs hooks (`hook.Run`, `hook.Call`,
  `Plugin.call`, `cw.plugin:Call`, ...) gets a `Hooks` page that defines
  them, with one entry per hook name: the arguments of its widest call,
  the functions that call it and every call site. The doc comment above a
  call documents the hook; without `@param` tags, the call's arguments are
  listed as the expressions they are.
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
  (`cw.currency.Class`). Aliases such as `local PLUGIN = PLUGIN` are not
  objects. Template objects that the loader injects (`PANEL`, `TOOL`, `ITEM`,
  `ENT`, ...) get one page per file, and an entity, weapon or effect folder
  (`init.lua`, `cl_init.lua`, `shared.lua`) gets one page named after the
  folder.
- **Realms.** Each function is marked server (blue), client (orange) or
  shared (both), from `@realm`, then `if SERVER` / `if CLIENT` blocks, then
  the file name. The marker is shown in the summary, the details, the
  sidebar and search results, and next to the title of a module whose
  functions all run on one side. A function defined once for the server and
  once for the client (such as in `sv_kernel.lua` and `cl_kernel.lua`) is
  shown once, as shared, with both definitions; when their doc comments
  differ, both are shown, the client's under "On the client".

A file's doc comment documents the module its `@module` names, else the
library it declares, else the only module its functions are in.

Local functions are private and never documented.

## Doc comments

A doc comment starts with `---` and continues on the following `--` lines. It
documents the declaration that directly follows it: a `function`, a
`name = function` assignment, a `class 'Name'` declaration, a top-level
`local NAME = ...` (such as `local PANEL = {}`), or a statement that runs a
hook (`hook.Run('Name', ...)`) or adds one (`hook.Add('Name', ...)`).

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
| `@param name[=default] [Type description]` | A parameter. The default marks it optional. |
| `@return [Type description, Type description]` | Return values; a comma followed by a type starts the next value. |
| `@variant name(a, b)` | An alternative signature. Indented `@param` and `@return` lines below it belong to the variant, and the text just above it describes it. A `@return` that is not indented applies to the whole function. |
| `@see [Reference]` or `@see Reference` | A related function or module (`Owner#name`, `Owner:name`, `Owner.name`, `name` or `Owner`). |
| `@warning [Label] text` | A warning box. The `Internal` label marks the function as internal. |
| `@deprecation [reason]` | Marks the function as deprecated. |
| `@deprecation_version [version]` | The version the deprecation starts at. |
| `@alias [Other.name]` | Another name the function is available under. |
| `@category [Name]` | On its own, starts a named section that groups the functions below it in the same file. The text after it describes the section. |
| `@realm [server\|client\|shared]` | Where the code runs, overriding the realm derived from the file name (`sv_`, `cl_`, `sh_`, `init.lua`, ...) and from `if SERVER` / `if CLIENT` blocks. |
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
