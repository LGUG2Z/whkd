# whkdrc for VS Code

Syntax highlighting for [whkd](https://github.com/LGUG2Z/whkd) configuration files.

- Keys, modifiers, `when` guards, blocks and `app =` / `layout =` conditions are highlighted, and mistakes the
  parser would reject (an unknown layout, an unknown condition, an unknown shell) are marked as invalid
- Commands are highlighted as PowerShell or batch, following the `.shell` directive
- Files named `whkdrc` or `.whkdrc`, or ending in `.whkdrc`, are recognised

The grammar in `syntaxes/` is generated from the parser by `cargo xtask editors` and should not be edited by hand.

## Trying it

```
code --extensionDevelopmentPath editors/vscode editors/fixtures/blocks.whkdrc
```

To install it, copy this folder to `%USERPROFILE%\.vscode\extensions\whkdrc` and restart VS Code.

## Tests

```
npm install
npm test
```

The tests tokenise `editors/fixtures` with the same engine VS Code uses and check the scopes. If VS Code is on the
`PATH` they also check that commands are really handed to the PowerShell and batch grammars (set
`WHKD_VSCODE_EXTENSIONS` to its `resources/app/extensions` folder otherwise).
