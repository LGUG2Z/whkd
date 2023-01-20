# whkd

_whkd_ is a simple hotkey daemon for Windows that reacts to input events by executing commands.

Its configuration file (`~/.config/whkdrc`) is a series of bindings which define the associations between the input events and the commands.

The format of the configuration file (and this project itself) is heavily inspired by `skhd` and `sxhkd`.

## Example

```
.shell pwsh # can be one of cmd | pwsh | powershell

# reload configuration
alt + o : taskkill /f /im whkd.exe && Start-Process whkd -WindowStyle hidden

# app shortcuts
alt + f : if ($wshell.AppActivate('Firefox') -eq $False) { start firefox }

# focus windows with komorebi
alt + h : komorebic focus left
alt + j : komorebic focus down
alt + k : komorebic focus up
alt + l : komorebic focus right
```
