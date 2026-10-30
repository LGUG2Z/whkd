//! A formatter for whkdrc files.
//!
//! The parser throws comments and blank lines away, so formatting works on the lines of the file
//! instead. Each line is lexed with the same rules the parser uses (so that normalising it cannot
//! change what it means), and the file is then laid out again. Nothing is ever reordered, merged
//! or split, and commands are kept byte for byte.
//!
//! Anything that does not parse, or that is shaped in a way this formatter does not understand
//! (for example two constructs on one line), is returned untouched.

use crate::continuation;
use crate::parse;
use crate::syntax::is_ident_continue;
use crate::syntax::is_ident_start;
use crate::syntax::is_value_char;
use crate::syntax::GESTURE_COOLDOWN;
use crate::syntax::PAUSE;
use crate::syntax::PAUSE_HOOK;
use crate::syntax::SHELL;
use crate::syntax::WHEEL_COOLDOWN;
use crate::syntax::WHEN;

/// Format the contents of a whkdrc. Input that does not parse is returned unchanged
#[must_use]
pub fn format(src: &str) -> String {
    if parse(src).is_err() {
        return src.to_string();
    }

    reformat(src).unwrap_or_else(|| src.to_string())
}

enum Item {
    Blank,
    Comment(String),
    Directive {
        content: String,
        comment: Option<String>,
    },
    /// `keys : command`, `keys when condition : command`, or `condition : command` in a block
    Binding {
        lhs: String,
        command: String,
        comment: Option<String>,
    },
    Open {
        keys: String,
        comment: Option<String>,
    },
    Close {
        comment: Option<String>,
    },
    /// A line of a command that the line before it continues with a `\`
    Continuation {
        command: String,
        comment: Option<String>,
    },
}

struct Line {
    item: Item,
    indent: usize,
    /// Lines in the same group are aligned with each other
    group: usize,
}

const INDENT: usize = 4;

fn reformat(src: &str) -> Option<String> {
    let (bom, body) = match src.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", src),
    };

    let crlf = body.matches("\r\n").count();
    let lf = body.matches('\n').count() - crlf;
    let eol = if crlf > lf { "\r\n" } else { "\n" };

    let mut lines: Vec<Line> = vec![];
    let mut in_block = false;
    let mut group = 0;
    let mut pending_blank = false;
    // whether the line before ends with a `\`, so that this one is part of its command
    let mut continuing = false;

    for raw in body.split('\n') {
        let line = raw.trim_end();
        let text = line.trim_start_matches([' ', '\t']);
        let indent = if in_block { INDENT } else { 0 };

        if text.is_empty() {
            continuing = false;
            pending_blank = !lines.is_empty();
            if !in_block {
                group += 1;
            }
            continue;
        }

        let item = if text.starts_with('#') {
            if !in_block {
                group += 1;
            }
            Item::Comment(text.to_string())
        } else if continuing {
            let (command, comment) = Lexer { rest: text }.command();
            Item::Continuation { command, comment }
        } else {
            classify(text, in_block)?
        };

        continuing = match &item {
            Item::Binding {
                command,
                comment: None,
                ..
            }
            | Item::Continuation {
                command,
                comment: None,
            } => continuation(command).is_some(),
            Item::Directive {
                content,
                comment: None,
            } => continuation(content).is_some(),
            _ => false,
        };

        if pending_blank {
            lines.push(Line {
                item: Item::Blank,
                indent: 0,
                group: 0,
            });
            pending_blank = false;
        }

        match item {
            Item::Open { .. } => {
                group += 1;
                lines.push(Line {
                    item,
                    indent,
                    group,
                });
                in_block = true;
                group += 1;
            }
            Item::Close { .. } => {
                lines.push(Line {
                    item,
                    indent: 0,
                    group,
                });
                in_block = false;
                group += 1;
            }
            Item::Continuation { .. } => lines.push(Line {
                item,
                indent: indent + INDENT,
                group,
            }),
            item => lines.push(Line {
                item,
                indent,
                group,
            }),
        }
    }

    if lines.is_empty() {
        return None;
    }

    // the widest left-hand side, and the widest directive that has a comment, in each group
    let mut lhs_width = vec![0; group + 2];
    let mut directive_width = vec![0; group + 2];
    for line in &lines {
        match &line.item {
            Item::Binding { lhs, .. } => {
                lhs_width[line.group] = lhs_width[line.group].max(lhs.chars().count());
            }
            Item::Directive {
                content,
                comment: Some(_),
            } => {
                directive_width[line.group] =
                    directive_width[line.group].max(content.chars().count());
            }
            _ => {}
        }
    }

    let mut out = String::from(bom);
    for line in &lines {
        let pad = " ".repeat(line.indent);
        let text = match &line.item {
            Item::Blank => String::new(),
            Item::Comment(comment) => format!("{pad}{comment}"),
            Item::Directive { content, comment } => match comment {
                Some(comment) => format!("{content:<0$} {comment}", directive_width[line.group]),
                None => content.clone(),
            },
            Item::Binding {
                lhs,
                command,
                comment,
            } => {
                let mut text = format!("{pad}{lhs:<0$} :", lhs_width[line.group]);
                for part in [Some(command), comment.as_ref()].into_iter().flatten() {
                    if !part.is_empty() {
                        text.push(' ');
                        text.push_str(part);
                    }
                }
                text
            }
            Item::Continuation { command, comment } => {
                with_comment(format!("{pad}{command}"), comment.as_ref())
            }
            Item::Open { keys, comment } => with_comment(format!("{keys} ["), comment.as_ref()),
            Item::Close { comment } => with_comment(String::from("]"), comment.as_ref()),
        };

        out.push_str(&text);
        out.push_str(eol);
    }

    Some(out)
}

fn with_comment(mut text: String, comment: Option<&String>) -> String {
    if let Some(comment) = comment {
        text.push(' ');
        text.push_str(comment);
    }
    text
}

/// Lex one non-blank, non-comment line (without its indentation), or `None` if it is not
/// something this formatter understands
fn classify(text: &str, in_block: bool) -> Option<Item> {
    let mut lx = Lexer { rest: text };

    if in_block {
        if lx.eat(']') {
            return Some(Item::Close {
                comment: lx.tail()?,
            });
        }

        let lhs = lx.condition()?;
        return lx.binding(lhs);
    }

    if lx.keyword(SHELL) {
        lx.rest = &lx.rest[SHELL.len()..];
        lx.skip_inline();
        let shell = lx.take_while(is_ident_continue);
        if shell.is_empty() {
            return None;
        }

        return Some(Item::Directive {
            content: format!("{SHELL} {shell}"),
            comment: lx.tail()?,
        });
    }

    if lx.keyword(PAUSE) {
        lx.rest = &lx.rest[PAUSE.len()..];
        lx.skip_inline();
        let keys = lx.hotkeys()?;

        return Some(Item::Directive {
            content: format!("{PAUSE} {keys}"),
            comment: lx.tail()?,
        });
    }

    for directive in [WHEEL_COOLDOWN, GESTURE_COOLDOWN] {
        if lx.keyword(directive) {
            lx.rest = &lx.rest[directive.len()..];
            lx.skip_inline();
            let milliseconds = lx.take_while(|c| c.is_ascii_digit());
            if milliseconds.is_empty() {
                return None;
            }

            return Some(Item::Directive {
                content: format!("{directive} {milliseconds}"),
                comment: lx.tail()?,
            });
        }
    }

    if lx.keyword(PAUSE_HOOK) {
        lx.rest = &lx.rest[PAUSE_HOOK.len()..];
        let (command, comment) = lx.command();
        let content = if command.is_empty() {
            String::from(PAUSE_HOOK)
        } else {
            format!("{PAUSE_HOOK} {command}")
        };

        return Some(Item::Directive { content, comment });
    }

    let keys = lx.hotkeys()?;

    if lx.eat('[') {
        return Some(Item::Open {
            keys,
            comment: lx.tail()?,
        });
    }

    if lx.keyword(WHEN) {
        lx.rest = &lx.rest[WHEN.len()..];
        lx.skip_inline();
        let condition = lx.condition()?;
        return lx.binding(format!("{keys} when {condition}"));
    }

    lx.binding(keys)
}

/// A cursor over one line, following the rules of the parser
struct Lexer<'a> {
    rest: &'a str,
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        match self.rest.strip_prefix(c) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> &'a str {
        let len = self.rest.find(|c| !predicate(c)).unwrap_or(self.rest.len());
        let (taken, rest) = self.rest.split_at(len);
        self.rest = rest;
        taken
    }

    fn skip_inline(&mut self) {
        self.take_while(|c| c == ' ' || c == '\t');
    }

    fn keyword(&self, name: &str) -> bool {
        self.rest
            .strip_prefix(name)
            .is_some_and(|after| !after.starts_with(is_ident_continue))
    }

    /// Nothing, or a comment, up to the end of the line
    fn tail(&mut self) -> Option<Option<String>> {
        self.skip_inline();

        if self.rest.is_empty() {
            Some(None)
        } else if self.rest.starts_with('#') {
            Some(Some(self.rest.trim_end().to_string()))
        } else {
            None
        }
    }

    /// The rest of the line up to a comment, and the comment
    fn command(&mut self) -> (String, Option<String>) {
        self.skip_inline();
        let command = self.take_while(|c| c != '#').trim_end().to_string();
        let comment = self
            .rest
            .starts_with('#')
            .then(|| self.rest.trim_end().to_string());
        (command, comment)
    }

    /// `:` and a command, for the binding whose left-hand side is `lhs`
    fn binding(&mut self, lhs: String) -> Option<Item> {
        self.skip_inline();
        if !self.eat(':') {
            return None;
        }

        let (command, comment) = self.command();
        Some(Item::Binding {
            lhs,
            command,
            comment,
        })
    }

    fn key(&mut self) -> Option<&'a str> {
        match self.peek()? {
            c if is_ident_start(c) => Some(self.take_while(is_ident_continue)),
            c if c.is_ascii_digit() => Some(self.take_while(|c| c.is_ascii_digit())),
            _ => None,
        }
    }

    /// Keys joined by ` + `, and `{...}` groups, which are kept as they are. Things that are next
    /// to each other, without a `+`, as `{_,shift +} {h,j}` is, stay next to each other, with one
    /// space between them if there was space
    fn hotkeys(&mut self) -> Option<String> {
        let mut out = String::new();
        let mut after_plus = false;

        loop {
            let before = self.rest.len();
            self.skip_inline();
            let spaced = self.rest.len() != before;

            let piece = match self.peek() {
                Some('+') => {
                    self.rest = &self.rest[1..];
                    out.truncate(out.trim_end().len());
                    out.push_str(" + ");
                    after_plus = true;
                    self.skip_inline();
                    continue;
                }
                Some('{') => {
                    let end = self.rest.find('}')? + 1;
                    let group = &self.rest[..end];
                    self.rest = &self.rest[end..];
                    group
                }
                // `when` ends the hotkey, unless it is a key, which it is after a `+`
                Some(_) if self.keyword(WHEN) && !out.is_empty() && !after_plus => break,
                Some(c) if is_ident_start(c) || c.is_ascii_digit() => self.key()?,
                _ => break,
            };

            if spaced && !after_plus && !out.is_empty() {
                out.push(' ');
            }

            out.push_str(piece);
            after_plus = false;
        }

        let keys = out.trim_end().to_string();
        (!keys.is_empty()).then_some(keys)
    }

    /// Terms joined by ` & `
    fn condition(&mut self) -> Option<String> {
        let mut terms = vec![self.term()?];

        loop {
            self.skip_inline();
            if !self.eat('&') {
                break;
            }

            self.skip_inline();
            terms.push(self.term()?);
        }

        Some(terms.join(" & "))
    }

    fn term(&mut self) -> Option<String> {
        if !self.peek().is_some_and(is_ident_start) {
            return None;
        }

        let mut words = vec![];
        loop {
            words.push(self.take_while(is_ident_continue));
            self.skip_inline();

            if !self.peek().is_some_and(is_ident_start) {
                break;
            }
        }

        let name = words.join(" ");

        let operator = if let Some(rest) = self.rest.strip_prefix("!=") {
            self.rest = rest;
            "!="
        } else if self.eat('=') {
            "="
        } else {
            return Some(name);
        };

        Some(format!("{name} {operator} {}", self.values()?))
    }

    /// Values joined by ` | `, each a run of words joined by a single space
    fn values(&mut self) -> Option<String> {
        let mut values = vec![];

        loop {
            self.skip_inline();
            let mut words = vec![];

            while self.peek().is_some_and(is_value_char) {
                words.push(self.take_while(is_value_char));
                self.skip_inline();
            }

            if words.is_empty() {
                return None;
            }

            values.push(words.join(" "));

            if !self.eat('|') {
                break;
            }
        }

        Some(values.join(" | "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn check(src: &str, expected: &str) {
        let output = format(src);
        assert_eq!(output, expected, "formatting\n{src:?}");
        assert_eq!(format(&output), output, "not idempotent for\n{src:?}");
        assert_eq!(parse(&output).unwrap(), parse(src).unwrap());
    }

    #[test]
    fn hotkey_spacing_is_normalised_and_case_kept() {
        check(
            ".shell cmd\nAlt+Shift   +\tH : x\n",
            ".shell cmd\nAlt + Shift + H : x\n",
        );
    }

    #[test]
    fn colons_align_within_a_group() {
        check(
            ".shell cmd\nalt + h : a\nalt + shift + h:b\nf1   :c\n",
            ".shell cmd\nalt + h         : a\nalt + shift + h : b\nf1              : c\n",
        );
    }

    #[test]
    fn groups_are_separated_by_blank_and_comment_lines() {
        check(
            ".shell cmd\nalt + h : a\nalt + shift + h : b\n\nf1 : c\n# note\nf2 : d\nf3     :  e\n",
            ".shell cmd\nalt + h         : a\nalt + shift + h : b\n\nf1 : c\n# note\nf2 : d\nf3 : e\n",
        );
    }

    #[test]
    fn when_guards_are_part_of_the_left_hand_side() {
        check(
            ".shell cmd\nalt + left : a\nalt+left   when layout=scrolling:b\nf1 when Firefox : c\n",
            ".shell cmd\nalt + left                         : a\nalt + left when layout = scrolling : b\nf1 when Firefox                    : c\n",
        );
    }

    #[test]
    fn a_key_named_when_is_a_key() {
        check(
            ".shell cmd\nalt+when:x\nalt + when when app=Code:y\n",
            ".shell cmd\nalt + when                 : x\nalt + when when app = Code : y\n",
        );
    }

    #[test]
    fn blocks() {
        check(
            ".shell cmd\nalt+n [ # open\n  # one\n\tFirefox:a\n  app=Zen Browser|Code   :b\nlayout=bsp|grid&app!=Code:c\n\n\n  Default : d\n]   # done\nf1:x\n",
            ".shell cmd\nalt + n [ # open\n    # one\n    Firefox                           : a\n    app = Zen Browser | Code          : b\n    layout = bsp | grid & app != Code : c\n\n    Default                           : d\n] # done\nf1 : x\n",
        );
    }

    #[test]
    fn bare_names_with_spaces_are_not_rewritten() {
        check(
            ".shell cmd\nf1 [\nGoogle   Chrome : a\n Default:b\n]\n",
            ".shell cmd\nf1 [\n    Google Chrome : a\n    Default       : b\n]\n",
        );
    }

    #[test]
    fn blocks_are_aligned_independently_of_each_other() {
        check(
            ".shell cmd\nf1 [\nA : a\nBBBB : b\n]\nf2 [\nC : c\n]\n",
            ".shell cmd\nf1 [\n    A    : a\n    BBBB : b\n]\nf2 [\n    C : c\n]\n",
        );
    }

    #[test]
    fn directives() {
        check(
            ".shell   pwsh   #c\n.pause alt+shift+p   # d\n.pause_hook   komorebic  toggle-pause   # e\n\nf1 : x\n",
            ".shell pwsh                         #c\n.pause alt + shift + p              # d\n.pause_hook komorebic  toggle-pause # e\n\nf1 : x\n",
        );
        check(
            ".shell cmd\n.pause alt+p\nf1 : x\n",
            ".shell cmd\n.pause alt + p\nf1 : x\n",
        );
        check(
            ".shell cmd\n.pause_hook\nf1 : x\n",
            ".shell cmd\n.pause_hook\nf1 : x\n",
        );
    }

    #[test]
    fn commands_are_kept_and_trailing_comments_stay_comments() {
        check(
            ".shell pwsh\nf1 :   echo   \"a  b\"   \t\nf2 : x#y\nf3 : x   #   y  \nf4 : # only\nf5 :   \n",
            ".shell pwsh\nf1 : echo   \"a  b\"\nf2 : x #y\nf3 : x #   y\nf4 : # only\nf5 :\n",
        );
    }

    #[test]
    fn empty_command_has_no_trailing_space() {
        check(
            ".shell cmd\nf1 :\nlonger : x\n",
            ".shell cmd\nf1     :\nlonger : x\n",
        );
    }

    #[test]
    fn blank_lines_collapse_and_edges_are_trimmed() {
        check(
            "\n\n  \n.shell cmd\n\n\n\n \t \nf1 : x\n\n\n\n",
            ".shell cmd\n\nf1 : x\n",
        );
    }

    #[test]
    fn comments_are_reindented_but_not_rewritten() {
        check(
            "   # top\n.shell cmd\n\t#   spaced  out  \nf1 [\n# in\n        #  deeper\n    Firefox : a\n  # before close\n]\n    # after\nf2 : x\n",
            "# top\n.shell cmd\n#   spaced  out\nf1 [\n    # in\n    #  deeper\n    Firefox : a\n    # before close\n]\n# after\nf2 : x\n",
        );
    }

    #[test]
    fn crlf_is_preserved() {
        check(
            ".shell cmd\r\nalt+h : a\r\n\r\n\r\nalt + shift + h:b # c\r\n",
            ".shell cmd\r\nalt + h : a\r\n\r\nalt + shift + h : b # c\r\n",
        );
    }

    #[test]
    fn mixed_line_endings_follow_the_majority() {
        let output = format(".shell cmd\r\nf1 : a\r\nf2 : b\nf3 : c\r\n");
        assert_eq!(output, ".shell cmd\r\nf1 : a\r\nf2 : b\r\nf3 : c\r\n");

        let output = format(".shell cmd\nf1 : a\r\nf2 : b\nf3 : c\n");
        assert_eq!(output, ".shell cmd\nf1 : a\nf2 : b\nf3 : c\n");
    }

    #[test]
    fn bom_is_preserved() {
        check("\u{feff}.shell cmd\nf1:x\n", "\u{feff}.shell cmd\nf1 : x\n");
        check(
            "\u{feff}\n\n# c\n.shell cmd\nf1:x\n",
            "\u{feff}# c\n.shell cmd\nf1 : x\n",
        );
    }

    #[test]
    fn missing_trailing_newline_is_added() {
        check(".shell cmd\nf1:x", ".shell cmd\nf1 : x\n");
        check(".shell cmd\nf1:x\r\n\r\n", ".shell cmd\r\nf1 : x\r\n");
    }

    #[test]
    fn tabs_are_whitespace() {
        check(
            ".shell\tcmd\nalt\t+\th\t:\techo\thi\n",
            ".shell cmd\nalt + h : echo\thi\n",
        );
    }

    #[test]
    fn unparseable_input_is_untouched() {
        let src = ".shell zsh\nf1:x\n\n\n";
        assert_eq!(format(src), src);
        assert_eq!(format(""), "");
    }

    #[test]
    fn lines_with_several_constructs_are_untouched() {
        // valid, but not something that can be laid out line by line
        let src = ".shell cmd\nf1 [ Firefox : a\n]\nf2   :   x\n";
        assert!(parse(src).is_ok());
        assert_eq!(format(src), src);
    }

    #[test]
    fn blocks_may_come_before_plain_bindings() {
        check(
            ".shell cmd\nf1 [\n Firefox:a\n]\nf2:b\nf1 when layout=bsp:c\n",
            ".shell cmd\nf1 [\n    Firefox : a\n]\nf2                   : b\nf1 when layout = bsp : c\n",
        );
    }

    const README: &str = r#".shell pwsh # can be one of cmd | pwsh | powershell
.pause alt + shift + p # can be any hotkey combo to toggle all other hotkeys on and off
.pause_hook echo "you can call whatever powershell command you want here"

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
"#;

    const CONDITIONS: &str = r"
.shell pwsh
alt + h [
    layout = bsp | grid         : komorebic focus left
    layout = columns            : komorebic cycle-focus previous
    app = Firefox & layout != rows : komorebic focus up
    Default                     : komorebic focus left
]
alt + left : komorebic stack left
alt + left when layout = scrolling : komorebic consume left
alt + right when app = Firefox | Zen Browser & layout != bsp | grid : x
";

    const BRIEF: &str = r"
.shell pwsh                      # required first; cmd | pwsh | powershell
.pause alt + p                   # optional
.pause_hook komorebic toggle-pause   # optional

alt + h : komorebic focus left   # plain binding
alt + left when layout = scrolling : komorebic consume left   # guarded binding
alt + n [                        # block
    Firefox                : echo hi        # bare process name
    app = Zen Browser | Code : echo hi
    layout = bsp | grid & app != Code : ...
    Default                : echo default
]
f1 :
alt + when : y
";

    const SEEDS: [&str; 5] = [
        README,
        CONDITIONS,
        BRIEF,
        ".shell cmd\r\n\r\nalt+h:a # c\r\n# x\r\nf1 [\r\n  Chrome : b\r\n]",
        "\u{feff}.shell cmd\n.pause f2\nf1 when Google  Chrome&layout!=rows:z\n\t# t\nf2 :\n",
    ];

    fn assert_good(src: &str) {
        let before = parse(src).unwrap();
        let output = format(src);
        assert_eq!(
            parse(&output).unwrap(),
            before,
            "semantics changed\n{src:?}\n{output:?}"
        );
        assert_eq!(
            format(&output),
            output,
            "not idempotent\n{src:?}\n{output:?}"
        );
    }

    #[test]
    fn representative_configs_round_trip() {
        for seed in SEEDS {
            assert_good(seed);
        }
    }

    #[test]
    fn the_readme_example_is_already_nearly_formatted() {
        let output = format(README);
        assert!(output.contains("alt + n [\n    # ProcessName"));
        assert!(output.contains("    Firefox       : echo \"hello firefox\"\n\n    # Spaces"));
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() % n
        }
    }

    const NOISE: [char; 24] = [
        ' ', '\t', '#', ':', '[', ']', '+', '&', '|', '=', '!', '\r', '\n', 'a', 'Z', '1', '_',
        '.', '-', '\u{feff}', '\u{a0}', 'é', ' ', '\n',
    ];

    fn mutate(rng: &mut Lcg, src: &str) -> String {
        let mut chars: Vec<char> = src.chars().collect();

        for _ in 0..=rng.below(3) {
            if chars.is_empty() {
                break;
            }

            let at = rng.below(chars.len());
            match rng.below(7) {
                0 => {
                    chars.remove(at);
                }
                1 => chars.insert(at, NOISE[rng.below(NOISE.len())]),
                2 => chars[at] = NOISE[rng.below(NOISE.len())],
                3 => {
                    // delete the line the character is on
                    let start = chars[..at]
                        .iter()
                        .rposition(|&c| c == '\n')
                        .map_or(0, |i| i + 1);
                    let end = chars[at..]
                        .iter()
                        .position(|&c| c == '\n')
                        .map_or(chars.len(), |i| at + i + 1);
                    chars.drain(start..end);
                }
                4 => {
                    // duplicate the line the character is on
                    let start = chars[..at]
                        .iter()
                        .rposition(|&c| c == '\n')
                        .map_or(0, |i| i + 1);
                    let end = chars[at..]
                        .iter()
                        .position(|&c| c == '\n')
                        .map_or(chars.len(), |i| at + i + 1);
                    let line: Vec<char> = chars[start..end].to_vec();
                    for (offset, c) in line.into_iter().enumerate() {
                        chars.insert(start + offset, c);
                    }
                }
                5 => {
                    // pad with whitespace
                    for _ in 0..=rng.below(4) {
                        chars.insert(at, if rng.below(2) == 0 { ' ' } else { '\t' });
                    }
                }
                _ => {
                    // swap two neighbouring characters
                    if at + 1 < chars.len() {
                        chars.swap(at, at + 1);
                    }
                }
            }
        }

        chars.into_iter().collect()
    }

    #[test]
    fn mutated_configs_keep_their_meaning() {
        let mut rng = Lcg(0x5eed_1234_abcd_ef01);
        let mut parsed = 0;
        let mut changed = 0;

        for i in 0..20_000 {
            let seed = SEEDS[i % SEEDS.len()];
            let mutated = mutate(&mut rng, seed);

            if parse(&mutated).is_ok() {
                parsed += 1;
                if format(&mutated) != mutated {
                    changed += 1;
                }
                assert_good(&mutated);
            }
        }

        assert!(parsed > 2_000, "only {parsed} mutations parsed");
        assert!(changed > 1_000, "only {changed} mutations were reformatted");
    }

    #[test]
    fn mouse_inputs_are_formatted_like_other_keys() {
        check(
            ".shell cmd\nAlt+WheelUp:x\nalt   +   xbutton1  when  layout=bsp:y\n",
            ".shell cmd\nAlt + WheelUp                    : x\nalt + xbutton1 when layout = bsp : y\n",
        );
    }

    #[test]
    fn the_wheel_cooldown_directive_is_formatted_like_the_others() {
        check(
            ".shell cmd\n.pause alt+p # a\n.wheel_cooldown    150   # b\nf1:x\n",
            ".shell cmd\n.pause alt + p      # a\n.wheel_cooldown 150 # b\nf1 : x\n",
        );
    }

    #[test]
    fn gestures_are_formatted_like_other_keys() {
        check(
            ".shell cmd\nAlt+Swipe3Left:x\n",
            ".shell cmd\nAlt + Swipe3Left : x\n",
        );
    }

    #[test]
    fn the_gesture_cooldown_directive_is_formatted_like_the_wheel_one() {
        check(
            ".shell cmd\n.wheel_cooldown 1 # a\n.gesture_cooldown   300   # b\nf1:x\n",
            ".shell cmd\n.wheel_cooldown 1     # a\n.gesture_cooldown 300 # b\nf1 : x\n",
        );
    }

    #[test]
    fn continued_commands_keep_their_lines_and_are_indented() {
        check(
            ".shell cmd\nalt+h:echo one \\\n      two \\\n  three\nf1:x\n",
            ".shell cmd\nalt + h : echo one \\\n    two \\\n    three\nf1      : x\n",
        );
    }

    #[test]
    fn continued_commands_in_blocks_and_the_pause_hook() {
        check(
            ".shell cmd\n.pause_hook   echo a \\\n b\nf1 [\n Code:echo c \\\n d \\\n\te\n Default:f\n]\n",
            ".shell cmd\n.pause_hook echo a \\\n    b\nf1 [\n    Code    : echo c \\\n        d \\\n        e\n    Default : f\n]\n",
        );
    }

    #[test]
    fn a_comment_ends_a_continued_command_and_is_kept() {
        check(
            ".shell cmd\nf1:echo one \\\n  two   # note\nf2:x\n",
            ".shell cmd\nf1 : echo one \\\n    two # note\nf2 : x\n",
        );
    }

    #[test]
    fn a_backslash_that_ends_a_word_does_not_continue() {
        check(
            ".shell cmd\nf1:explorer C:\\Users\\\nf2   :x\n",
            ".shell cmd\nf1 : explorer C:\\Users\\\nf2 : x\n",
        );
    }

    #[test]
    fn a_continuation_that_continues_onto_nothing_still_formats() {
        check(
            ".shell cmd\nf1:echo \\\n\nf2:x\n",
            ".shell cmd\nf1 : echo \\\n\nf2 : x\n",
        );
    }

    #[test]
    fn continued_commands_keep_crlf_line_endings() {
        check(
            ".shell cmd\r\nf1:echo one \\\r\n two\r\nf2:x\r\n",
            ".shell cmd\r\nf1 : echo one \\\r\n    two\r\nf2 : x\r\n",
        );
    }

    #[test]
    fn a_continuation_line_that_looks_like_a_binding_is_not_one() {
        // `f2 : y` is a continuation of the command of `f1`, so it is not aligned with anything
        check(
            ".shell cmd\nf1:echo \\\n  f2 : y\nlonger_key:z\n",
            ".shell cmd\nf1         : echo \\\n    f2 : y\nlonger_key : z\n",
        );
    }

    #[test]
    fn groups_in_a_hotkey_are_kept_as_they_are() {
        check(
            ".shell cmd\nalt+{h,j}:echo {a,b}\nalt+{ h ,  j }:echo {a,b}\n",
            ".shell cmd\nalt + {h,j}      : echo {a,b}\nalt + { h ,  j } : echo {a,b}\n",
        );
    }

    #[test]
    fn groups_next_to_each_other_stay_next_to_each_other() {
        check(
            ".shell cmd\nalt+{_,shift +}   {h,j}:x\nalt + {_,shift +}{h,j}:x\n",
            ".shell cmd\nalt + {_,shift +} {h,j} : x\nalt + {_,shift +}{h,j}  : x\n",
        );
    }

    #[test]
    fn spacing_around_a_plus_next_to_a_group_is_normalised() {
        check(
            ".shell cmd\n{alt,ctrl}+h:x\n{alt,ctrl}  +  {h,j}:x\n",
            ".shell cmd\n{alt,ctrl} + h     : x\n{alt,ctrl} + {h,j} : x\n",
        );
    }

    #[test]
    fn groups_and_guards_and_blocks() {
        check(
            ".shell cmd\nalt+{h,j}   when   layout=bsp:echo {a,b}\n{a,b} when Firefox:x\nalt+{k,l}[\n Firefox:echo {c,d}\n  Default:echo {e,f}\n]\n",
            ".shell cmd\nalt + {h,j} when layout = bsp : echo {a,b}\n{a,b} when Firefox            : x\nalt + {k,l} [\n    Firefox : echo {c,d}\n    Default : echo {e,f}\n]\n",
        );
    }

    #[test]
    fn a_key_named_when_after_a_group_is_a_key() {
        check(
            ".shell cmd\n{a,b}+when:x\n",
            ".shell cmd\n{a,b} + when : x\n",
        );
    }

    #[test]
    fn braces_in_commands_are_left_alone() {
        check(
            ".shell pwsh\nalt+h:Get-Process | % { $_.Name }   # note\nalt + {h,j}:echo \\{x\\} {a,b}\n",
            ".shell pwsh\nalt + h     : Get-Process | % { $_.Name } # note\nalt + {h,j} : echo \\{x\\} {a,b}\n",
        );
    }

    #[test]
    fn expansions_that_continue_onto_the_next_line_are_formatted_as_commands() {
        check(
            ".shell cmd\n{alt,ctrl}+{h,j}:echo {a,b} \\\n {c,d}\n",
            ".shell cmd\n{alt,ctrl} + {h,j} : echo {a,b} \\\n    {c,d}\n",
        );
    }
}
