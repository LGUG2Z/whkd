# tree-sitter-whkdrc

A [tree-sitter](https://tree-sitter.github.io) grammar for [whkd](https://github.com/LGUG2Z/whkd) configuration
files, with highlight, injection and fold queries.

`grammar.js` and everything in `queries/` are generated from the parser by `cargo xtask editors` and should not be
edited by hand. `src/` is generated from `grammar.js` by `tree-sitter generate` and is committed so that editors can
build the parser without Node.

- Keys, modifiers, `when` guards, blocks and `app =` / `layout =` conditions are parsed and highlighted
- Layout and shell values are part of the grammar, so an unknown one is a syntax error, like it is for `whkd validate`
- Commands are injected as PowerShell (`.shell pwsh` / `.shell powershell`) or batch (`.shell cmd`), following the
  `.shell` directive. The injection query pairs the directive with each command, which needs a tree-sitter that
  supports sibling patterns in injection queries (Neovim does; see below)

## Regenerating

After the language changes in Rust (a new layout, keyword or key name):

```
cargo xtask editors
cd editors/tree-sitter-whkdrc
npm install
npx tree-sitter generate
```

`cargo test` fails if the generated files are out of date.

## Tests

```
npm test                                   # tree-sitter test: corpus and highlight assertions
npx tree-sitter build -o whkdrc.dll        # then, to check the queries in Neovim's own engine:
nvim --headless -l test/neovim.lua whkdrc.dll ../fixtures/basic.whkdrc ../fixtures/cmd.whkdrc
```

The grammar is also checked against the real parser by a differential test: the fixtures and thousands of mutations of
them are parsed by both, and they must agree on whether the input is valid, apart from a short list of documented
differences (things a grammar cannot express, such as a word boundary after a keyword). It needs `nvim` and the
tree-sitter CLI, so it is ignored by default:

```
cargo test -p xtask -- --ignored
```

## Using it

**Neovim** (verified with 0.11): build the parser, put it and the queries on the runtime path, and associate the file
name, since `whkdrc` has no extension:

```lua
vim.filetype.add({
  filename = { whkdrc = 'whkdrc', ['.whkdrc'] = 'whkdrc' },
  extension = { whkdrc = 'whkdrc' },
})
vim.treesitter.language.add('whkdrc', { path = '/path/to/whkdrc.dll' })
-- copy queries/*.scm to <runtimepath>/queries/whkdrc/
vim.api.nvim_create_autocmd('FileType', { pattern = 'whkdrc', callback = function() vim.treesitter.start() end })
```

The injection queries use the language names `powershell` and `batch`, so those parsers need to be installed too.

**Helix** and **Zed**: the grammar lives in a subdirectory of the whkd repository (`editors/tree-sitter-whkdrc`), so
point the grammar source at it with a `subpath` (Helix) or the equivalent. These have not been tried yet.
