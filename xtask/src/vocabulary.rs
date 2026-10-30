//! Everything the editor grammars need to know about the language, read from the code that
//! implements it rather than copied out of it.

use serde_json::json;
use serde_json::Value;
use whkd_core::GestureInput;
use whkd_core::Layout;
use whkd_core::MouseInput;
use whkd_core::Shell;
use whkd_parser::syntax;

/// What a shell's commands are written in, which decides the language embedded in them
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ShellFamily {
    PowerShell,
    Batch,
}

/// An exhaustive match, so that adding a shell forces a decision about how its commands are
/// highlighted
pub fn shell_family(shell: Shell) -> ShellFamily {
    match shell {
        Shell::Pwsh | Shell::Powershell => ShellFamily::PowerShell,
        Shell::Cmd => ShellFamily::Batch,
    }
}

/// The names of the shells whose commands are written in `family`
pub fn shells_in(family: ShellFamily) -> Vec<String> {
    Shell::ALL
        .into_iter()
        .filter(|shell| shell_family(*shell) == family)
        .map(|shell| shell.name().to_string())
        .collect()
}

pub struct Vocabulary {
    pub directives: Vec<String>,
    pub shells: Vec<String>,
    pub keywords: Vec<String>,
    pub predicates: Vec<String>,
    pub layouts: Vec<LayoutName>,
    pub modifiers: Vec<String>,
    /// The buttons and scroll directions that can be the last key of a hotkey
    pub mouse_inputs: Vec<String>,
    /// The swipes that can be the last key of a hotkey
    pub gestures: Vec<String>,
    /// Every keyboard key that is not a modifier
    pub keys: Vec<String>,
}

pub struct LayoutName {
    pub name: String,
    /// Matches every spelling `Layout::parse` accepts
    pub pattern: String,
}

impl Vocabulary {
    pub fn load() -> Self {
        let modifiers = whkd_keys::modifiers();
        let mouse_inputs: Vec<String> = MouseInput::ALL
            .map(|input| input.name().to_string())
            .into_iter()
            .collect();
        let gestures: Vec<String> = GestureInput::ALL
            .map(|input| input.name().to_string())
            .into_iter()
            .collect();
        let keys = whkd_keys::names()
            .into_iter()
            .filter(|name| {
                !modifiers.contains(name)
                    && !mouse_inputs.contains(name)
                    && !gestures.contains(name)
            })
            .collect();

        Self {
            directives: syntax::DIRECTIVES.map(String::from).to_vec(),
            shells: Shell::ALL.map(|shell| shell.name().to_string()).to_vec(),
            keywords: syntax::KEYWORDS.map(String::from).to_vec(),
            predicates: syntax::PREDICATES.map(String::from).to_vec(),
            layouts: Layout::ALL
                .map(|layout| LayoutName {
                    name: layout.name().to_string(),
                    pattern: layout_pattern(layout),
                })
                .into_iter()
                .collect(),
            modifiers,
            mouse_inputs,
            gestures,
            keys,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "$comment": crate::GENERATED_NOTICE,
            "directives": self.directives,
            "shells": self.shells,
            "keywords": self.keywords,
            "predicates": self.predicates,
            "operators": {
                "comment": syntax::COMMENT.to_string(),
                "key_separator": syntax::KEY_SEPARATOR.to_string(),
                "delimiter": syntax::DELIMITER.to_string(),
                "block_open": syntax::BLOCK_OPEN.to_string(),
                "block_close": syntax::BLOCK_CLOSE.to_string(),
                "equals": syntax::EQUALS.to_string(),
                "not_equals": syntax::NOT_EQUALS,
                "or": syntax::OR.to_string(),
                "and": syntax::AND.to_string(),
            },
            "layouts": self
                .layouts
                .iter()
                .map(|layout| json!({ "name": layout.name, "pattern": layout.pattern }))
                .collect::<Vec<_>>(),
            "modifiers": self.modifiers,
            "mouse_inputs": self.mouse_inputs,
            "gestures": self.gestures,
            "keys": self.keys,
        })
    }
}

/// A pattern for a layout that accepts the same spellings as `Layout::parse`, which ignores case
/// and separators (so `bsp`, `BSP`, `ultrawide-vertical-stack` and `UltrawideVerticalStack` are all
/// the same layout). It is spelt out with character classes rather than a case-insensitive flag so
/// that it means the same thing to every regex engine the editors use.
pub fn layout_pattern(layout: Layout) -> String {
    layout
        .name()
        .split('-')
        .map(case_insensitive)
        .collect::<Vec<_>>()
        .join("[-_ ]?")
}

fn case_insensitive(word: &str) -> String {
    word.chars()
        .map(|c| {
            if c.is_alphabetic() {
                format!(
                    "[{}{}]",
                    c.to_lowercase().next().unwrap_or(c),
                    c.to_uppercase().next().unwrap_or(c)
                )
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    /// Every spelling of a layout that the parser accepts
    fn spellings(layout: Layout) -> Vec<String> {
        let parts: Vec<&str> = layout.name().split('-').collect();
        let pascal: String = parts
            .iter()
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect();

        vec![
            layout.name().to_string(),
            layout.name().to_uppercase(),
            pascal,
            parts.concat(),
            parts.join("_"),
            parts.join(" "),
            parts.concat().to_uppercase(),
        ]
    }

    #[test]
    fn layout_patterns_match_every_spelling_the_parser_accepts() {
        for layout in Layout::ALL {
            let pattern = Regex::new(&format!("^(?:{})$", layout_pattern(layout))).unwrap();

            for spelling in spellings(layout) {
                assert_eq!(
                    Layout::parse(&spelling),
                    Some(layout),
                    "the parser should accept {spelling}"
                );
                assert!(
                    pattern.is_match(&spelling),
                    "{} should match {spelling}",
                    pattern.as_str()
                );
            }
        }
    }

    #[test]
    fn layout_patterns_reject_what_the_parser_rejects() {
        for layout in Layout::ALL {
            let pattern = Regex::new(&format!("^(?:{})$", layout_pattern(layout))).unwrap();
            let name = layout.name();

            // a separator inside a word, or doubled, or trailing, is not a spelling of the layout
            for misspelt in [
                format!("{}_{}", &name[..1], &name[1..]),
                format!("{name}_"),
                format!("-{name}"),
                format!("{name}.x"),
                name.replace('-', "--"),
            ] {
                // a layout of one word has no hyphen to double
                if misspelt == name {
                    continue;
                }

                assert_eq!(Layout::parse(&misspelt), None, "{misspelt}");
                assert!(!pattern.is_match(&misspelt), "{misspelt}");
            }
        }
    }

    #[test]
    fn layout_patterns_do_not_match_other_layouts() {
        for layout in Layout::ALL {
            let pattern = Regex::new(&format!("^(?:{})$", layout_pattern(layout))).unwrap();

            for other in Layout::ALL.into_iter().filter(|other| *other != layout) {
                assert!(!pattern.is_match(other.name()), "{layout} matched {other}");
            }
        }
    }

    #[test]
    fn vocabulary_is_read_from_the_code() {
        let vocabulary = Vocabulary::load();
        assert_eq!(
            vocabulary.directives,
            vec![
                ".shell",
                ".pause",
                ".pause_hook",
                ".wheel_cooldown",
                ".gesture_cooldown",
                ".ignore"
            ]
        );
        assert_eq!(vocabulary.shells, vec!["cmd", "pwsh", "powershell"]);
        assert_eq!(vocabulary.layouts.len(), Layout::ALL.len());
        assert!(vocabulary.modifiers.contains(&String::from("alt")));
        assert!(!vocabulary.keys.contains(&String::from("alt")));
        assert!(vocabulary.keys.contains(&String::from("return")));
    }
}
