use crate::whkdrc::Shell;
use crate::whkdrc::Whkdrc;
use chumsky::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub keys: Vec<String>,
    pub command: String,
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

    let command = take_until(choice((comment, text::newline())))
        .padded()
        .map(|c| c.0)
        .collect::<String>();

    let binding = hotkeys.then_ignore(delimiter).then(command);

    shell
        .then(
            binding
                .map(|(keys, command)| HotkeyBinding { keys, command })
                .padded()
                .padded_by(comment.repeated())
                .repeated()
                .at_least(1),
        )
        .map(|(shell, bindings)| Whkdrc { shell, bindings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let src = r#"
.shell cmd

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
            bindings: vec![
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("h")],
                    command: String::from("komorebic focus left"),
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("j")],
                    command: String::from("komorebic focus down"),
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("k")],
                    command: String::from("komorebic focus up"),
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("l")],
                    command: String::from("komorebic focus right"),
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("1")],
                    command: String::from("komorebic focus-workspace 0"),
                },
            ],
        };

        assert_eq!(output.unwrap(), expected);
    }
}
