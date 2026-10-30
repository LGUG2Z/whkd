# whkdrc for Zed

Syntax highlighting for [whkd](https://github.com/LGUG2Z/whkd) configuration files, using the tree-sitter grammar in
`editors/tree-sitter-whkdrc`.

- Keys, modifiers, `{a,b}` groups, `when` guards, blocks, `.ignore`, `.when` conditions and the `\` that continues a
  command are highlighted
- Commands are highlighted as PowerShell, following the `.shell` directive. The command of a `.when` condition is
  always highlighted as batch, as it is always run in `cmd`. Zed has no batch language of its own, so `.shell cmd` and
  `.when` commands are not highlighted unless an extension for one is installed
- Files named `whkdrc`, or ending in `.whkdrc`, are recognised

`languages/whkdrc/highlights.scm` and `injections.scm` are generated from the parser by `cargo xtask editors` and should
not be edited by hand.

## Installing it

1. Open the command palette and run `zed: install dev extension`
2. Choose this folder (`editors/zed`)

Zed builds the grammar itself, from the repository and revision in `extension.toml`:

```toml
[grammars.whkdrc]
repository = "https://github.com/LGUG2Z/whkd"
rev = "..."
path = "editors/tree-sitter-whkdrc"
```

`rev` is the commit that has the grammar the queries here were generated for, so it has to be changed whenever the
grammar changes, and the commit has to be one that the repository has. To try a grammar that is not pushed, use a copy
of this folder in which `repository` is a `file:///` URL for your clone of whkd.

Two things are not highlighted: a condition defined with `.when` is a process name to the grammar, as it cannot know
which names are defined, and an unknown condition is not marked as a mistake. `whkd validate` reports them.
