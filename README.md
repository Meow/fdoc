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
| `--title <NAME>` | Project name (default: the `name` in `packagespec.lua`, else the directory name) |
| `--source-url <URL>` | Base URL that source paths are appended to for "view source" links |
| `--exclude <NAME>` | Directory name to skip; repeatable (default: `docs`, `.git`) |
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

Files are grouped by where they live in the source tree:

- `packages/<name>/...` becomes a package, named after its `packagespec.lua`;
- `plugins/<name>/...` and `plugins/<file>.lua` become plugins, named through
  `PLUGIN:set_name`;
- everything else is the core of the project.

Inside each group, functions are collected into modules by the table they are
defined on: `function Foo.Bar:baz()` lands in `Foo.Bar`. Metatable locals such
as `local player_meta = FindMetaTable('Player')` are shown as `Player`,
`GM` hooks and the plugin object (`PLUGIN` or the name given to
`PLUGIN:set_global`) get their own pages, free functions are listed under
`Globals`, and per-file template objects (`PANEL`, `TOOL`, `CMD`, `SKIN`,
`THEME`, `ENT`, ...) get one page per file, named after `vgui.Register` where
available.

Local functions are private and never documented.

## Doc comments

A doc comment starts with `---` and continues on the following `--` lines. It
documents the declaration that directly follows it: a `function`, a
`name = function` assignment or a `class 'Name'` declaration.

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
| `@variant name(a, b)` | An alternative signature. Indented `@param` lines below it belong to the variant, and the text just above it describes it. |
| `@see [Reference]` or `@see Reference` | A related function or module (`Owner#name`, `Owner:name`, `Owner.name`, `name` or `Owner`). |
| `@warning [Label] text` | A warning box. The `Internal` label marks the function as internal. |
| `@deprecation [reason]` | Marks the function as deprecated. |
| `@deprecation_version [version]` | The version the deprecation starts at. |
| `@alias [Other.name]` | Another name the function is available under. |
| `@category [Name]` | On its own, starts a named section that groups the functions below it in the same file. The text after it describes the section. |
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
