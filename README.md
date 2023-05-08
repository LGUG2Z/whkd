# whkd

_whkd_ is a simple hotkey daemon for Windows that reacts to input events by executing commands.

Its configuration file (`whkdrc`) is a series of bindings which define the associations between the input events and the commands.
By default, this file should be located in `~/.config/`, or an alternative can be set using the environment variable `WHKD_CONFIG_HOME`.

The format of the configuration file (and this project itself) is heavily inspired by `skhd` and `sxhkd`.

## Example

```
.shell pwsh # can be one of cmd | pwsh | powershell

# Specify different behaviour depending on the app
alt + n [
    # ProcessName as shown by `Get-Process`
    Firefox       : echo "hello firefox"
    
    # Spaces are fine, no quotes required
    Google Chrome : echo "hello chrome"
]

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
