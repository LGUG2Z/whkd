# whkd

_whkd_ is a simple hotkey daemon for Windows that reacts to input events by executing commands.

Its configuration file (`whkdrc`) is a series of bindings which define the associations between the input events and the
commands. By default, this file should be located in `~/.config/`, or an alternative can be set using the environment
variable `WHKD_CONFIG_HOME`.

If you are compiling from the `master` branch, a `--config` flag is also available, which, when used, overrides the
`WHKD_CONFIG_HOME` environment variable. This flag will be made available in v0.1.3+.

The format of the configuration file (and this project itself) is heavily inspired by `skhd` and `sxhkd`.

## Example

```
.shell pwsh # can be one of cmd | pwsh | powershell
.pause alt + shift + p # can be any hotkey combo to toggle all other hotkeys on and off

# Specify different behaviour depending on the app
# These "app : command" style bindings MUST come immediately below the .shell directive
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
