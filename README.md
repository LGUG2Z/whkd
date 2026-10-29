# whkd

_whkd_ is a simple hotkey daemon for Windows that reacts to input events by executing commands.

Its configuration file (`whkdrc`) is a series of bindings which define the associations between the input events and the
commands. By default, this file should be located in `~/.config/`, or an alternative can be set using the environment
variable `WHKD_CONFIG_HOME`.

If you are compiling from the `master` branch, a `--config` flag is also available, which, when used, overrides the
`WHKD_CONFIG_HOME` environment variable. This flag will be made available in v0.1.3+.

You can check a configuration file for syntax errors and invalid key names without starting the daemon by running
`whkd validate`, which uses the default lookup described above, or `whkd validate path\to\whkdrc` for an explicit path.
It exits with a non-zero status if the file is not valid.

The format of the configuration file (and this project itself) is heavily inspired by `skhd` and `sxhkd`.

## Example

```
.shell pwsh # can be one of cmd | pwsh | powershell
.pause alt + shift + p # can be any hotkey combo to toggle all other hotkeys on and off
.pause_hook echo "you can call whatever powershell command you want here"

# Specify different behaviour depending on the app
# These "app : command" style blocks can go anywhere below the directives
alt + n [
    # ProcessName as shown by `Get-Process`
    Firefox       : echo "hello firefox"
    
    # Spaces are fine, no quotes required
    Google Chrome : echo "hello chrome"
]

alt + q [
    # Default is a keyword which will apply to all apps
    # If you only have Default, this is the same as doing "alt + q : komorebic close"
    Default       : komorebic close

    # Ignore is a keyword which will skip running the hotkey for the given process
    Google Chrome : Ignore
]

# focus windows with komorebi
alt + h : komorebic focus left
alt + j : komorebic focus down
alt + k : komorebic focus up
alt + l : komorebic focus right
```

## Conditions

Each line inside a `[ ... ]` block is a condition followed by a command. A bare process name is shorthand for
`app = <name>`, so existing configuration files keep working. Conditions can also test the layout of the focused
komorebi workspace:

```
alt + h [
    layout = bsp | grid            : komorebic focus left
    layout = columns               : komorebic cycle-focus previous
    app = Firefox & layout != rows : komorebic focus up
    Default                        : komorebic focus left
]
```

- `app = a | b` and `layout = x | y` match any of the listed values, and `!=` negates them
- `&` requires every condition on a line to hold
- The first line that matches wins, and `Default` is only used if nothing else matches, wherever it appears
- Layouts are komorebi's built-in ones: `bsp`, `columns`, `rows`, `vertical-stack`, `horizontal-stack`,
  `ultrawide-vertical-stack`, `grid`, `right-main-vertical-stack` and `scrolling`. Custom layouts are not supported.
- komorebi is only asked for its layout when a `layout` condition is in play for the key that was pressed. If komorebi
  is not running, `layout` conditions do not match (including `!=`), and whkd prints a warning once

### Overriding a binding with `when`

A block is not needed to add a condition to a single binding. Put `when` and a condition (written exactly as in a
block, so `|`, `&` and `!=` all work) after the keys, next to the binding it overrides:

```
alt + left : komorebic stack left
alt + left when layout = scrolling : komorebic consume left
```

All of the bindings for the same hotkey, whether plain, `when` or in a block, are decided together when it is pressed.
They are tried in file order and the first match wins. The plain binding is the fallback, so it can sit anywhere in the
file. An explicit `Default` line in a block is preferred over a plain binding for the same hotkey.

### Checking a configuration

`whkd validate` checks the syntax, key names and conditions, including layout names, without needing komorebi to be
running. It also reports things that parse but are probably a mistake:

- a hotkey bound more than once with no condition, so only one of them can ever run
- an entry that can never be reached because an earlier one matches whenever it does
- a binding with an empty command
- (as advice) a hotkey that only has conditional bindings and no default, which swallows the key when none match

## Formatting

`whkd fmt` formats a configuration file, using the same lookup as `whkd validate` (the default location, the global
`--config` flag, or an explicit `whkd fmt path\to\whkdrc`). By default the formatted file is printed to stdout.

```
whkd fmt                  # print the formatted default config
whkd fmt --write          # rewrite the file in place (only if something changed), also -w
whkd fmt --check          # print nothing; exit 1 and name the file on stderr if it is not formatted
```

`--write` and `--check` cannot be combined. A file that does not parse is reported exactly as `whkd validate` reports it
and is never modified, and the formatted output is re-parsed and compared with the original before it is used, so
formatting can never change what a configuration means. Comments and blank lines are kept. It normalizes:

- hotkeys to `alt + shift + h` (key spelling and case are kept)
- the `:` of each run of consecutive bindings (a `when` guard counts as part of the
  hotkey) to a single column; runs are separated by blank lines or comments and are aligned independently
- `[ ... ]` blocks: entries indented by 4 spaces with their `:` aligned across the block, `]` at column 0, and
  conditions written as `layout = bsp | grid & app != Code`
- one space after directives, with trailing comments of consecutive directives aligned
- more than one blank line to one, trailing whitespace, and the end of the file (a single trailing newline); comment
  lines are re-indented to the block they are in but not otherwise changed
- line endings (CRLF or LF, whichever the file mostly uses) and a leading byte order mark are preserved

Bindings are never reordered, merged or split, and commands are not touched. Lines the formatter does not understand
(for example several constructs on one line) cause the file to be left as it is.

## Mouse and trackpad bindings

A mouse button or a scroll direction can trigger a binding, as the last key of a hotkey, held with keyboard modifiers:

```
alt + wheelup   : komorebic focus left
alt + wheeldown : komorebic focus right
alt + mbutton   : komorebic toggle-monocle
alt + xbutton1  : komorebic cycle-workspace previous

# they take conditions like any other binding
alt + wheelup when layout = scrolling : komorebic scroll-left
alt + shift + wheeldown [
    app = Firefox : echo only in firefox
    Default       : echo everywhere else
]
```

| Name | Input |
|---|---|
| `lbutton`, `rbutton`, `mbutton` | left, right and middle button |
| `xbutton1`, `xbutton2` | the back and forward buttons |
| `wheelup`, `wheeldown` | scrolling up and down |
| `wheelleft`, `wheelright` | scrolling sideways |

Names are not case sensitive, and `whkd validate` suggests the right one for a typo or a synonym (`scrollup`,
`mouse3`, ...).

- A mouse input has to be the **last** key, and only alt, ctrl, shift and win can be held with it. Their left and right
  versions are the same, and they must match exactly: `alt + wheelup` does not fire with shift held as well
- `lbutton` and `rbutton` need a modifier. Binding a plain click would swallow every click, leaving no way to fix the
  config
- `.pause` has to be a keyboard hotkey, and pausing whkd pauses the mouse bindings too
- A button fires when it is **pressed**, and its release is swallowed as well, so that an application never sees a
  release without a press
- Like keyboard hotkeys, a mouse input that has a binding is swallowed: it is not delivered to the window under the
  cursor, even if none of the conditions match. Use a plain binding, or a `Default` entry, to say what happens then
- `app =` is the focused window, as it is for keyboard bindings, not the window under the cursor
- Input that other programs synthesise is ignored, so a command that sends a click or a scroll cannot trigger itself
- Nothing listens for the mouse unless the config has a mouse binding

### Trackpads

Two-finger scrolling on a precision touchpad arrives as wheel events, so the `wheel` bindings work with a trackpad as
they do with a mouse. A trackpad scrolls in small steps, so they are added up, and a binding fires once for each full
notch (120 units) of scrolling in one direction. What is left over counts towards the next one, and counting starts
again after half a second without scrolling, when the direction changes, or when the modifiers change.

### Cooldown

A trackpad keeps scrolling for a moment after the fingers lift, so one flick could fire a binding such as
`cycle-workspace next` several times. After a wheel binding fires, scrolling in the same direction is ignored for a
cooldown, 100 milliseconds unless the whkdrc says otherwise. It is set with a directive, after the other directives:

```
.shell pwsh
.pause alt + p
.wheel_cooldown 200 # milliseconds, 0 turns it off
```

- It counts from the moment the binding fired, so scrolling that carries on still fires every so often, rather than
  never again
- What is scrolled during the cooldown is forgotten, and still swallowed
- Scrolling the other way, and the other wheel (sideways), have their own cooldown, so reversing is never held back
- It applies to the wheel only: a button fires once for each press
- It is a single setting for all wheel bindings. A mouse wheel that clicks in steps may want it lower or off, a trackpad
  higher

Gestures with three or four fingers (swipes, taps) are not something Windows passes on to programs. Windows Settings
can map them to keyboard shortcuts, and whkd can bind those.

### Limits

- Like the keyboard hook, the mouse hook does not see input sent to windows running as administrator unless whkd does
  too
- If you use another program that handles the mouse (komorebi's focus follows mouse, window dragging), pick modifier
  and button combinations it does not use
- A held mouse button as a modifier (`lbutton + h`) and drag bindings are not supported

## Editor support

`whkdrc` has no file extension, so editors need to be told what it is. The files in `editors/` are generated from the
parser itself (`cargo xtask editors`), so a new layout, keyword or key name reaches them without being copied by hand,
and `cargo xtask editors --check` (which also runs as a test) fails if they are out of date.

- **VS Code**: `editors/vscode` is an extension that recognises files named `whkdrc` or `.whkdrc` and highlights keys,
  modifiers, `when` guards, blocks and conditions. Commands are highlighted as PowerShell or batch depending on the
  `.shell` directive. See `editors/vscode/README.md`.
- **Neovim, Helix, Zed and Emacs** can use the tree-sitter grammar in `editors/tree-sitter-whkdrc`, which also follows
  the `.shell` directive for commands. It is checked against the real parser by a differential test. See
  `editors/tree-sitter-whkdrc/README.md`.
- **Other editors that read TextMate grammars** (Sublime Text, the JetBrains IDEs, `bat`, Shiki) can use
  `editors/vscode/syntaxes/whkdrc.tmLanguage.json`; the scope name is `source.whkdrc`.

## Valid Hotkeys
The list of valid hotkeys can be found [here](https://docs.rs/win-hotkeys/latest/win_hotkeys/enum.VKey.html).

## License

`whkd` is licensed under the [Komorebi 2.0.0 license](./LICENSE.md), which
is a fork of the [PolyForm Strict 1.0.0
license](https://polyformproject.org/licenses/strict/1.0.0). On a high level
this means that you are free to do whatever you want with `whkd` for
personal use other than redistribution, or distribution of new works (i.e.
hard-forks) based on the software.

Anyone is free to make their own fork of `whkd` with changes intended
either for personal use or for integration back upstream via pull requests.

The [Komorebi 2.0.0 License](./LICENSE.md) does not permit any kind of
commercial use.

### Licensing for Commercial Use

A dedicated Individual Commercial Use License is available for those who want to use `whkd` at work.

The [`komorebi` Individual Commercial Use License](https://lgug2z.com/software/komorebi) adds “Commercial Use” as a
“Permitted Use” for the licensed individual only, for the duration of a valid paid license subscription only, for both
`komorebi` and `whkd`. All provisions and restrictions enumerated in
the [Komorebi License](https://github.com/LGUG2Z/komorebi-license)
continue to apply.

### Contribution Licensing

Contributions are accepted with the following understanding:

- Contributed content is licensed under the terms of the 0-BSD license
- Contributors accept the terms of the project license at the time of contribution

By making a contribution, you accept both the current project license terms, and that all contributions that you have
made are provided under the terms of the 0-BSD license.

#### Zero-Clause BSD

```
Permission to use, copy, modify, and/or distribute this software for
any purpose with or without fee is hereby granted.

THE SOFTWARE IS PROVIDED “AS IS” AND THE AUTHOR DISCLAIMS ALL
WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES
OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE
FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY
DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT
OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```
