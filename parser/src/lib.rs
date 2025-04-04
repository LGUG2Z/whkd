use chumsky::prelude::*;
use whkd_core::HotkeyBinding;
use whkd_core::Shell;
use whkd_core::Whkdrc;

#[allow(clippy::too_many_lines)]
#[must_use]
pub fn parser() -> impl Parser<char, Whkdrc, Error = Simple<char>> {
    let comment = just::<_, _, Simple<char>>("#")
        .then(take_until(text::newline()))
        .padded()
        .ignored();

    let shell = just(".shell")
        .padded()
        .ignore_then(choice((just("pwsh"), just("powershell"), just("cmd"))))
        .repeated()
        .exactly(1)
        .collect::<String>()
        .map(Shell::from);

    let hotkeys = choice((text::ident(), text::int(10)))
        .padded()
        .separated_by(just("+"))
        .at_least(1)
        .collect::<Vec<String>>();

    let command = take_until(choice((comment, text::newline(), end())))
        .padded()
        .map(|c| c.0)
        .collect::<String>();

    let pause = just(".pause")
        .padded()
        .ignore_then(hotkeys)
        .padded_by(comment.repeated())
        .or_not();

    let pause_hook = just(".pause_hook")
        .padded()
        .ignore_then(command.clone())
        .padded_by(comment.repeated())
        .or_not();

    let delimiter = just(":").padded();

    let default_keyword = just("Default").padded();
    let ignore_keyword = just("Ignore").padded();

    let process_name = choice((
        default_keyword.map(|_| String::from("Default")),
        text::ident()
            .padded()
            .repeated()
            .at_least(1)
            .map(|a| a.join(" ")),
    ));

    let process_command = choice((
        ignore_keyword.map(|_| String::from("Ignore")),
        command.clone(),
    ));

    let process_mapping = process_name
        .then_ignore(delimiter)
        .then(process_command)
        .padded()
        .padded_by(comment.repeated())
        .repeated()
        .at_least(1);

    let process_command_map = just("[")
        .ignore_then(process_mapping)
        .padded()
        .padded_by(comment.repeated())
        .then_ignore(just("]"))
        .collect::<Vec<(String, String)>>();

    let binding = hotkeys.then_ignore(delimiter).then(command);
    let process_bindings = hotkeys.then(process_command_map);

    comment
        .repeated()
        .ignore_then(shell)
        .then(pause)
        .then(pause_hook)
        .then(
            process_bindings
                .map(|(keys, apps_commands)| {
                    let mut collected = vec![];
                    for (app, command) in apps_commands {
                        collected.push(HotkeyBinding {
                            keys: keys.clone(),
                            command,
                            process_name: Option::from(app),
                        });
                    }

                    (keys, collected)
                })
                .padded()
                .padded_by(comment.repeated())
                .repeated()
                .at_least(0),
        )
        .then(
            binding
                .map(|(keys, command)| HotkeyBinding {
                    keys,
                    command,
                    process_name: None,
                })
                .padded()
                .padded_by(comment.repeated())
                .repeated()
                .at_least(1),
        )
        .map(
            |((((shell, pause_binding), pause_hook), app_bindings), bindings)| Whkdrc {
                shell,
                app_bindings,
                bindings,
                pause_binding,
                pause_hook,
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_line_parse() {
        let src = r#"
.shell pwsh # can be one of cmd | pwsh | powershell

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from("echo \"Hello\""),
                process_name: None,
            }],
            pause_binding: None,
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_starts_with_comment_single_line_parser() {
        let src = r#"
# sample comment
.shell pwsh # can be one of cmd | pwsh | powershell

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from("echo \"Hello\""),
                process_name: None,
            }],
            pause_binding: None,
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_parse() {
        let src = r#"
.shell cmd

# Specify different behaviour depending on the app
alt + n [
    # ProcessName as shown by `Get-Process`
    Firefox       : echo "hello firefox"

    # Spaces are fine, no quotes required
    Google Chrome : echo "hello chrome"
]

# leading newlines are fine
# line comments should parse and be ignored
alt + h     : komorebic focus left # so should comments at the end of a line
alt + j     : komorebic focus down
alt + k     : komorebic focus up
alt + l     : komorebic focus right

# so should empty lines
alt + 1 : komorebic focus-workspace 0 # digits are fine in the hotkeys section

# trailing newlines are fine


"#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Cmd,
            app_bindings: vec![(
                vec![String::from("alt"), String::from("n")],
                vec![
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello firefox""#),
                        process_name: Option::from("Firefox".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        process_name: Option::from("Google Chrome".to_string()),
                    },
                ],
            )],
            bindings: vec![
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("h")],
                    command: String::from("komorebic focus left"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("j")],
                    command: String::from("komorebic focus down"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("k")],
                    command: String::from("komorebic focus up"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("l")],
                    command: String::from("komorebic focus right"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("1")],
                    command: String::from("komorebic focus-workspace 0"),
                    process_name: None,
                },
            ],
            pause_binding: None,
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_binding_without_modkeys() {
        let src = r#"
# sample comment
.shell pwsh # can be one of cmd | pwsh | powershell

f11 : echo "hello f11""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("f11")],
                command: String::from("echo \"hello f11\""),
                process_name: None,
            }],
            pause_binding: None,
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_default_and_scoped_ignores() {
        let src = r#"
.shell pwsh

alt + n [
    Default       : echo "hello world"
    Firefox       : echo "hello firefox"
    Google Chrome : echo "hello chrome"
    Zen Browser   : Ignore
]

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![(
                vec![String::from("alt"), String::from("n")],
                vec![
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello world""#),
                        process_name: Option::from("Default".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello firefox""#),
                        process_name: Option::from("Firefox".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        process_name: Option::from("Google Chrome".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from("Ignore"),
                        process_name: Option::from("Zen Browser".to_string()),
                    },
                ],
            )],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                process_name: None,
            }],
            pause_binding: None,
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hotkey() {
        let src = r#"
.shell pwsh
.pause ctrl + shift + esc

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                process_name: None,
            }],
            pause_binding: Some(vec![
                "ctrl".to_string(),
                "shift".to_string(),
                "esc".to_string(),
            ]),
            pause_hook: None,
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hook() {
        let src = r#"
.shell pwsh
.pause ctrl + shift + esc
.pause_hook komorebic toggle-pause

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                process_name: None,
            }],
            pause_binding: Some(vec![
                "ctrl".to_string(),
                "shift".to_string(),
                "esc".to_string(),
            ]),
            pause_hook: Some("komorebic toggle-pause".to_string()),
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hook_with_comments() {
        let src = r#"
.shell pwsh                            # can be one of cmd | pwsh | powershell
.pause alt + shift + p                 # can be any hotkey combo to toggle all other hotkeys on and off
.pause_hook komorebic toggle-pause     # another comment

alt + h : echo "Hello""#;

        let output = parser().parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                process_name: None,
            }],
            pause_binding: Some(vec![
                "alt".to_string(),
                "shift".to_string(),
                "p".to_string(),
            ]),
            pause_hook: Some("komorebic toggle-pause".to_string()),
        };

        assert_eq!(output.unwrap(), expected);
    }
}
