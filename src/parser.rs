use crate::whkdrc::Shell;
use crate::whkdrc::Whkdrc;
use chumsky::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub keys: Vec<String>,
    pub command: String,
    pub process_name: Option<String>,
}

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
        .at_least(2)
        .collect::<Vec<String>>();

    let delimiter = just(":").padded();

    let command = take_until(choice((comment, text::newline(), end())))
        .padded()
        .map(|c| c.0)
        .collect::<String>();

    let process_name = text::ident()
        .padded()
        .repeated()
        .at_least(1)
        .map(|a| a.join(" "));

    let process_mapping = process_name
        .then_ignore(delimiter)
        .then(command.clone())
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

    shell
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
        .map(|((shell, app_bindings), bindings)| Whkdrc {
            shell,
            app_bindings,
            bindings,
        })
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
        };

        assert_eq!(output.unwrap(), expected);
    }
}
