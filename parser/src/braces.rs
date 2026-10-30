//! Brace expansion: `{a,b}` and `{1-9}` in a hotkey and in its command stand for several bindings.
//!
//! ```text
//! alt + {_,shift +} {h,j,k,l} : komorebic {focus,move} {left,down,up,right}
//! ```
//!
//! is eight bindings. Every `{...}` group in the hotkey has as many elements as the group at the same
//! position in the command, and the bindings are made by taking one element from every group:
//! the first group changes the slowest. A command may have fewer groups than the hotkey, in which
//! case the extra groups only multiply the hotkey.
//!
//! This module is the part that knows nothing about keys. The parser turns each hotkey that comes
//! out of [`expand_hotkey`] into keys as it would one written out in full, so the keys, their
//! suggestions and the errors for them are the same as for a hotkey without braces.

use crate::syntax::EMPTY_ELEMENT;
use crate::syntax::ESCAPE;
use crate::syntax::GROUP_CLOSE;
use crate::syntax::GROUP_OPEN;
use crate::syntax::GROUP_SEPARATOR;
use crate::syntax::RANGE_SEPARATOR;
use std::ops::Range;

/// A hotkey that stands for more bindings than this is a mistake, such as `{1-9}{1-9}{a-z}{a-z}`
pub const MAX_EXPANSIONS: usize = 512;

/// What is wrong with a group, where in the text that was given (in bytes from its start)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupError {
    pub message: String,
    pub span: Range<usize>,
    pub label: String,
    pub help: Option<String>,
}

impl GroupError {
    fn new(message: impl Into<String>, span: Range<usize>, label: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
            label: label.into(),
            help: None,
        }
    }

    fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// One thing a group can be replaced by
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub text: String,
    /// Where it was written. Every element of a range has the span of the range
    pub span: Range<usize>,
}

/// `{a,b,c}`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub span: Range<usize>,
    pub elements: Vec<Element>,
}

/// Read the group that `text` starts with, which has to start with `{`: how long it is in bytes,
/// and its elements. Whitespace around an element does not count, `_` is nothing, and `a-e` is
/// `a`, `b`, `c`, `d` and `e`
pub fn scan_group(text: &str) -> Result<(usize, Vec<Element>), GroupError> {
    debug_assert!(text.starts_with(GROUP_OPEN));

    let mut elements = vec![];
    let mut start = GROUP_OPEN.len_utf8();
    let mut chars = text.char_indices().skip(1).peekable();

    loop {
        let Some((at, c)) = chars.next() else {
            return Err(unterminated(text));
        };

        match c {
            GROUP_OPEN => {
                return Err(GroupError::new(
                    "groups cannot be nested",
                    at..at + 1,
                    format!("`{GROUP_OPEN}` inside a group"),
                )
                .with_help(format!(
                    "write the elements of the inner group out, or escape a literal brace as `{ESCAPE}{GROUP_OPEN}`"
                )));
            }
            '\n' | '\r' => return Err(unterminated(text)),
            GROUP_SEPARATOR | GROUP_CLOSE => {
                let element = &text[start..at];
                add_element(&mut elements, element, start..at)?;
                start = at + c.len_utf8();

                if c == GROUP_CLOSE {
                    return Ok((start, elements));
                }
            }
            _ => {}
        }
    }
}

fn unterminated(text: &str) -> GroupError {
    GroupError::new(
        format!("unterminated `{GROUP_OPEN}`"),
        0..GROUP_OPEN.len_utf8().min(text.len()),
        format!("expected `{GROUP_CLOSE}` before the end of the line"),
    )
}

/// An element is made of everything between the separators, trimmed
fn add_element(
    elements: &mut Vec<Element>,
    raw: &str,
    span: Range<usize>,
) -> Result<(), GroupError> {
    let text = raw.trim();
    let lead = raw.len() - raw.trim_start().len();
    let span = if text.is_empty() {
        span
    } else {
        span.start + lead..span.start + lead + text.len()
    };

    if text.is_empty() {
        return Err(
            GroupError::new("empty element", span, "nothing between the separators").with_help(
                format!("write `{EMPTY_ELEMENT}` for an element that is nothing"),
            ),
        );
    }

    if text == EMPTY_ELEMENT {
        elements.push(Element {
            text: String::new(),
            span,
        });
        return Ok(());
    }

    let chars: Vec<char> = text.chars().collect();
    if let [from, separator, to] = chars[..] {
        if separator == RANGE_SEPARATOR
            && from.is_ascii_alphanumeric()
            && to.is_ascii_alphanumeric()
        {
            let same_kind = (from.is_ascii_digit() && to.is_ascii_digit())
                || (from.is_ascii_lowercase() && to.is_ascii_lowercase())
                || (from.is_ascii_uppercase() && to.is_ascii_uppercase());

            if !same_kind || from > to {
                return Err(GroupError::new(
                    format!("invalid range `{text}`"),
                    span,
                    if same_kind {
                        "this range goes backwards"
                    } else {
                        "a range is of digits, of lowercase letters or of uppercase letters"
                    },
                )
                .with_help("a range goes up, as in `1-9`, `a-f` or `A-F`"));
            }

            for c in from..=to {
                elements.push(Element {
                    text: c.to_string(),
                    span: span.clone(),
                });
            }

            return Ok(());
        }
    }

    elements.push(Element {
        text: text.to_string(),
        span,
    });
    Ok(())
}

/// Every way of choosing one element from each group, as an index into each, the first group
/// changing the slowest. Nothing is chosen from no groups, once
#[must_use]
pub fn choices(lengths: &[usize]) -> Vec<Vec<usize>> {
    let mut all = vec![vec![]];

    for &length in lengths {
        all = all
            .into_iter()
            .flat_map(|choice| {
                (0..length).map(move |index| {
                    let mut next = choice.clone();
                    next.push(index);
                    next
                })
            })
            .collect();
    }

    all
}

/// How many bindings groups of these lengths make, or `None` if that is more than can be counted
#[must_use]
pub fn expansions(lengths: &[usize]) -> Option<usize> {
    lengths
        .iter()
        .try_fold(1usize, |count, &length| count.checked_mul(length))
}

/// A part of a hotkey: the text between the groups, and the groups
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text { text: String, start: usize },
    Group(Group),
}

/// A part of the expanded text, and where it comes from in the file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub expanded: Range<usize>,
    pub source: Range<usize>,
    /// Whether the two are the same text, so that a position in one is the position in the other.
    /// An element that came from a range is not, as `3` is not in `1-9`
    pub exact: bool,
}

/// A hotkey with one element chosen from each group
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    pub segments: Vec<Segment>,
}

/// The hotkey `pieces` make when `choice` says which element to take from each group
#[must_use]
pub fn expand_hotkey(pieces: &[Piece], choice: &[usize]) -> Expanded {
    let mut text = String::new();
    let mut segments = vec![];
    let mut groups = choice.iter();

    for piece in pieces {
        let (part, source, exact) = match piece {
            Piece::Text { text, start } => (text.as_str(), *start..*start + text.len(), true),
            Piece::Group(group) => {
                let index = *groups.next().expect("a choice for every group");
                let element = &group.elements[index];
                let single = group
                    .elements
                    .iter()
                    .filter(|other| other.span == element.span)
                    .count()
                    == 1;
                (element.text.as_str(), element.span.clone(), single)
            }
        };

        segments.push(Segment {
            expanded: text.len()..text.len() + part.len(),
            source,
            exact,
        });
        text.push_str(part);
    }

    Expanded { text, segments }
}

impl Expanded {
    /// Where in the file the characters of `span`, a span of the expanded text, are written. Text
    /// that was taken from an element is the element, as far as that goes, and anything that is
    /// not anywhere (the end of the text, nothing at all) is `whole`
    #[must_use]
    pub fn locate(&self, span: &Range<usize>, whole: &Range<usize>) -> Range<usize> {
        let find = |offset: usize| {
            self.segments
                .iter()
                .find(|segment| segment.expanded.contains(&offset))
        };

        let (Some(first), Some(last)) = (
            find(span.start),
            find(span.end.saturating_sub(1).max(span.start)),
        ) else {
            return whole.clone();
        };

        let start = if first.exact {
            first.source.start + (span.start - first.expanded.start)
        } else {
            first.source.start
        };

        let last_offset = span.end.saturating_sub(1).max(span.start);
        let end = if last.exact {
            last.source.start + (last_offset - last.expanded.start) + 1
        } else {
            last.source.end
        };

        start..end.max(start + 1)
    }
}

/// What is wrong with the command of a binding with groups, with spans into the command
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub message: String,
    pub span: Range<usize>,
    pub label: String,
    pub help: Option<String>,
    /// The group of the hotkey that it is not like
    pub hotkey_group: Option<(usize, String)>,
}

impl CommandError {
    fn from_group(error: GroupError) -> Self {
        Self {
            message: error.message,
            span: error.span,
            label: error.label,
            help: error.help.or_else(|| {
                Some(format!(
                    "write `{ESCAPE}{GROUP_OPEN}` for a brace that is part of the command"
                ))
            }),
            hotkey_group: None,
        }
    }
}

/// The commands that `command` stands for, one for each of `choices` (what is chosen from each
/// group of the hotkey, whose groups are `hotkey`). `\{` and `\}` are braces that are not groups
pub fn expand_command(
    command: &str,
    hotkey: &[Group],
    choices: &[Vec<usize>],
) -> Result<Vec<String>, CommandError> {
    enum Part {
        Text(String),
        Group(Vec<Element>),
    }

    let mut parts = vec![];
    let mut text = String::new();
    let mut at = 0;
    let mut groups = 0;

    while at < command.len() {
        let rest = &command[at..];
        let c = rest.chars().next().expect("not at the end");

        if c == ESCAPE && rest[1..].starts_with([GROUP_OPEN, GROUP_CLOSE]) {
            text.push(rest[1..].chars().next().expect("a brace"));
            at += 2;
        } else if c == GROUP_OPEN {
            let (length, elements) = scan_group(rest).map_err(|error| {
                CommandError::from_group(GroupError {
                    span: error.span.start + at..error.span.end + at,
                    ..error
                })
            })?;

            if groups == hotkey.len() {
                return Err(CommandError {
                    message: format!(
                        "this command has {} groups, and the hotkey has {}",
                        groups + 1,
                        hotkey.len()
                    ),
                    span: at..at + length,
                    label: String::from("there is no group in the hotkey for this one"),
                    help: Some(format!(
                        "the groups of the command take the place of those of the hotkey in order; write `{ESCAPE}{GROUP_OPEN}` for a brace that is part of the command"
                    )),
                    hotkey_group: None,
                });
            }

            let expected = hotkey[groups].elements.len();
            if elements.len() != expected {
                return Err(CommandError {
                    message: format!(
                        "this group has {} elements, but the group of the hotkey it goes with has {expected}",
                        elements.len()
                    ),
                    span: at..at + length,
                    label: format!("expected {expected} elements"),
                    help: Some(format!(
                        "every group of the command takes the place of the group at the same position in the hotkey; write `{ESCAPE}{GROUP_OPEN}` for a brace that is part of the command"
                    )),
                    hotkey_group: Some((groups, format!("{expected} elements"))),
                });
            }

            parts.push(Part::Text(std::mem::take(&mut text)));
            parts.push(Part::Group(elements));
            groups += 1;
            at += length;
        } else {
            text.push(c);
            at += c.len_utf8();
        }
    }

    parts.push(Part::Text(text));

    Ok(choices
        .iter()
        .map(|choice| {
            let mut next = 0;
            let mut out = String::new();

            for part in &parts {
                match part {
                    Part::Text(text) => out.push_str(text),
                    Part::Group(elements) => {
                        out.push_str(&elements[choice[next]].text);
                        next += 1;
                    }
                }
            }

            out
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements(text: &str) -> Vec<String> {
        scan_group(text)
            .unwrap()
            .1
            .into_iter()
            .map(|element| element.text)
            .collect()
    }

    fn error(text: &str) -> GroupError {
        scan_group(text).unwrap_err()
    }

    #[test]
    fn a_group_is_its_elements() {
        assert_eq!(elements("{h,j,k,l}"), ["h", "j", "k", "l"]);
        assert_eq!(elements("{a}"), ["a"]);
        assert_eq!(scan_group("{a,b} rest").unwrap().0, 5);
    }

    #[test]
    fn whitespace_around_an_element_does_not_count_but_inside_it_does() {
        assert_eq!(elements("{ left ,  right }"), ["left", "right"]);
        assert_eq!(elements("{_, shift +}"), ["", "shift +"]);
        assert_eq!(elements("{one two,three}"), ["one two", "three"]);
    }

    #[test]
    fn an_underscore_is_nothing() {
        assert_eq!(elements("{_,shift}"), ["", "shift"]);
        assert_eq!(elements("{_}"), [""]);
        // only on its own
        assert_eq!(elements("{a_,_b}"), ["a_", "_b"]);
    }

    #[test]
    fn ranges_of_digits_and_letters_are_every_character() {
        assert_eq!(elements("{1-9}").len(), 9);
        assert_eq!(elements("{1-3}"), ["1", "2", "3"]);
        assert_eq!(elements("{a-d}"), ["a", "b", "c", "d"]);
        assert_eq!(elements("{A-C}"), ["A", "B", "C"]);
        assert_eq!(elements("{5-5}"), ["5"]);
        // and mix with other elements
        assert_eq!(elements("{0,2-4,x}"), ["0", "2", "3", "4", "x"]);
    }

    #[test]
    fn something_that_is_not_a_range_is_text() {
        assert_eq!(elements("{left-right}"), ["left-right"]);
        assert_eq!(elements("{a-bc}"), ["a-bc"]);
        assert_eq!(elements("{-}"), ["-"]);
        assert_eq!(elements("{a--}"), ["a--"]);
    }

    #[test]
    fn a_bad_range_says_what_is_wrong() {
        let backwards = error("{9-1}");
        assert_eq!(backwards.message, "invalid range `9-1`");
        assert_eq!(backwards.label, "this range goes backwards");
        assert_eq!(backwards.span, 1..4);

        let mixed = error("{a-9}");
        assert_eq!(mixed.message, "invalid range `a-9`");
        assert!(mixed.label.contains("digits"), "{}", mixed.label);

        assert_eq!(error("{a-Z}").message, "invalid range `a-Z`");
        assert!(error("{9-1}").help.unwrap().contains("goes up"));
    }

    #[test]
    fn empty_elements_are_mistakes() {
        for text in ["{}", "{a,}", "{,a}", "{a,,b}", "{ }"] {
            let found = error(text);
            assert_eq!(found.message, "empty element", "{text}");
            assert!(found.help.unwrap().contains('_'), "{text}");
        }

        assert_eq!(error("{a,,b}").span, 3..3);
    }

    #[test]
    fn unterminated_and_nested_groups_are_mistakes() {
        for text in ["{a,b", "{", "{a,b\nc}", "{a,b\r\n}"] {
            let found = error(text);
            assert_eq!(found.message, "unterminated `{`", "{text:?}");
            assert_eq!(found.span, 0..1, "{text:?}");
        }

        let nested = error("{a,{b,c}}");
        assert_eq!(nested.message, "groups cannot be nested");
        assert_eq!(nested.span, 3..4);
    }

    #[test]
    fn element_spans_are_where_they_were_written() {
        let (_, found) = scan_group("{ ab , c}").unwrap();
        assert_eq!(found[0].span, 2..4);
        assert_eq!(found[1].span, 7..8);

        // every element of a range is the range
        let (_, found) = scan_group("{1-3}").unwrap();
        assert!(found.iter().all(|element| element.span == (1..4)));
    }

    #[test]
    fn unicode_is_kept() {
        assert_eq!(elements("{é,ß}"), ["é", "ß"]);
        assert_eq!(error("{é,}").span, 4..4);
    }

    #[test]
    fn choices_change_the_first_group_the_slowest() {
        assert_eq!(choices(&[]), vec![Vec::<usize>::new()]);
        assert_eq!(choices(&[2]), [[0], [1]]);
        assert_eq!(
            choices(&[2, 3]),
            [[0, 0], [0, 1], [0, 2], [1, 0], [1, 1], [1, 2]]
        );
        assert_eq!(choices(&[2, 2, 2]).len(), 8);
        assert!(choices(&[2, 0]).is_empty());
    }

    #[test]
    fn expansions_are_counted_without_overflowing() {
        assert_eq!(expansions(&[]), Some(1));
        assert_eq!(expansions(&[2, 4]), Some(8));
        assert_eq!(expansions(&[usize::MAX, 2]), None);
    }

    fn group(at: usize, text: &str) -> Group {
        let (length, mut found) = scan_group(text).unwrap();
        for element in &mut found {
            element.span = element.span.start + at..element.span.end + at;
        }
        Group {
            span: at..at + length,
            elements: found,
        }
    }

    fn pieces() -> Vec<Piece> {
        // `alt + {_,shift +} {h,j}` written from the start of the file
        vec![
            Piece::Text {
                text: String::from("alt + "),
                start: 0,
            },
            Piece::Group(group(6, "{_,shift +}")),
            Piece::Text {
                text: String::from(" "),
                start: 17,
            },
            Piece::Group(group(18, "{h,j}")),
        ]
    }

    #[test]
    fn a_hotkey_is_expanded_in_place() {
        let pieces = pieces();
        let text = |choice: &[usize]| expand_hotkey(&pieces, choice).text;

        assert_eq!(text(&[0, 0]), "alt +  h");
        assert_eq!(text(&[0, 1]), "alt +  j");
        assert_eq!(text(&[1, 0]), "alt + shift + h");
        assert_eq!(text(&[1, 1]), "alt + shift + j");
    }

    #[test]
    fn a_span_of_the_expansion_is_found_in_the_file() {
        let pieces = pieces();
        let whole = 0..23;
        let expanded = expand_hotkey(&pieces, &[1, 1]);
        assert_eq!(expanded.text, "alt + shift + j");

        // text that was written out is where it was written
        assert_eq!(expanded.locate(&(0..3), &whole), 0..3);
        assert_eq!(expanded.locate(&(2..5), &whole), 2..5);
        // an element is the element it is
        assert_eq!(expanded.locate(&(6..11), &whole), 9..14);
        assert_eq!(expanded.locate(&(8..9), &whole), 11..12);
        assert_eq!(expanded.locate(&(14..15), &whole), 21..22);
        // nothing is anywhere in particular
        assert_eq!(expanded.locate(&(15..15), &whole), whole);
        assert_eq!(expanded.locate(&(40..41), &whole), whole);
    }

    #[test]
    fn a_character_of_a_range_is_the_whole_range() {
        let pieces = vec![
            Piece::Text {
                text: String::from("alt + "),
                start: 0,
            },
            Piece::Group(group(6, "{1-9}")),
        ];
        let expanded = expand_hotkey(&pieces, &[3]);
        assert_eq!(expanded.text, "alt + 4");
        assert_eq!(expanded.locate(&(6..7), &(0..11)), 7..10);
    }

    fn hotkey_groups() -> Vec<Group> {
        vec![group(0, "{_,shift +}"), group(12, "{h,j,k,l}")]
    }

    fn all(command: &str) -> Result<Vec<String>, CommandError> {
        let groups = hotkey_groups();
        let lengths: Vec<usize> = groups.iter().map(|g| g.elements.len()).collect();
        expand_command(command, &groups, &choices(&lengths))
    }

    #[test]
    fn a_command_takes_the_elements_in_the_same_places() {
        let found = all("komorebic {focus,move} {left,down,up,right}").unwrap();
        assert_eq!(found.len(), 8);
        assert_eq!(found[0], "komorebic focus left");
        assert_eq!(found[1], "komorebic focus down");
        assert_eq!(found[3], "komorebic focus right");
        assert_eq!(found[4], "komorebic move left");
        assert_eq!(found[7], "komorebic move right");
    }

    #[test]
    fn a_command_may_leave_groups_of_the_hotkey_out() {
        // the second group of the hotkey only multiplies the bindings
        let found = expand_command("echo {a,b}", &hotkey_groups(), &choices(&[2, 4])).unwrap();
        assert_eq!(found.len(), 8);
        assert_eq!(found[0], "echo a");
        assert_eq!(found[3], "echo a");
        assert_eq!(found[4], "echo b");

        // and a command with no groups is the same for all of them
        let same = all("echo hello").unwrap();
        assert_eq!(same.len(), 8);
        assert!(same.iter().all(|command| command == "echo hello"));
    }

    #[test]
    fn a_range_counts_as_all_its_elements() {
        let groups = vec![group(0, "{1-3}")];
        let found = expand_command("workspace {0-2}", &groups, &choices(&[3])).unwrap();
        assert_eq!(found, ["workspace 0", "workspace 1", "workspace 2"]);
    }

    #[test]
    fn an_escaped_brace_is_a_brace() {
        let found = all(r"echo \{a\} {focus,move}{_,x}").unwrap_err();
        // the second group of the command has 2 elements where the hotkey's has 4
        assert!(found.message.contains("2 elements"), "{}", found.message);

        let groups = vec![group(0, "{a,b}")];
        let found = expand_command(r"Get-Item \{x\} {1,2}", &groups, &choices(&[2])).unwrap();
        assert_eq!(found, ["Get-Item {x} 1", "Get-Item {x} 2"]);
    }

    #[test]
    fn too_many_groups_in_a_command_is_a_mistake() {
        let error = all("echo {a,b} {a,b,c,d} {x,y}").unwrap_err();
        assert_eq!(
            error.message,
            "this command has 3 groups, and the hotkey has 2"
        );
        assert_eq!(error.span, 21..26);
        assert!(error.help.unwrap().contains(r"\{"));
    }

    #[test]
    fn a_group_that_is_not_as_long_is_a_mistake() {
        let error = all("echo {a,b,c}").unwrap_err();
        assert_eq!(
            error.message,
            "this group has 3 elements, but the group of the hotkey it goes with has 2"
        );
        assert_eq!(error.span, 5..12);
        assert_eq!(error.label, "expected 2 elements");
        assert_eq!(error.hotkey_group, Some((0, String::from("2 elements"))));
    }

    #[test]
    fn a_broken_group_in_a_command_is_found_in_the_command() {
        let error = all("echo foo {a,").unwrap_err();
        assert_eq!(error.message, "unterminated `{`");
        assert_eq!(error.span, 9..10);
        assert!(error.help.unwrap().contains(r"\{"));

        let error = all("echo {a,,b}").unwrap_err();
        assert_eq!(error.message, "empty element");
        assert_eq!(error.span, 8..8);
    }

    #[test]
    fn a_command_without_a_group_of_its_own_to_expand_is_not_touched() {
        let groups = vec![group(0, "{a,b}")];
        let found = expand_command("echo é ß", &groups, &choices(&[2])).unwrap();
        assert_eq!(found, ["echo é ß", "echo é ß"]);

        // a backslash that does not escape a brace is a backslash
        let found = expand_command(
            r"explorer C:\Users\ {a,b}",
            &[group(0, "{x,y}")],
            &choices(&[2]),
        )
        .unwrap();
        assert_eq!(found, [r"explorer C:\Users\ a", r"explorer C:\Users\ b"]);
    }
}
