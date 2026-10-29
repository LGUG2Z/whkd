//! The key names a whkdrc may use, as `win-hotkeys` understands them.
//!
//! This is the one place that knows them, so `whkd` (to flag typos with a suggestion) and the
//! editor grammar generator (to highlight keys, and modifiers separately) cannot disagree.

use whkd_core::GestureInput;
use whkd_core::MouseInput;
use whkd_parser::KeyValidator;
use win_hotkeys::VKey;

/// Spellings `win-hotkeys` accepts that its own `Display` does not produce
const ALIASES: [&str; 9] = [
    "alt", "ctrl", "win", "lalt", "ralt", "lctrl", "rctrl", "lwin", "rwin",
];

/// Names people commonly try that are not valid, and the valid name to suggest instead
pub const SYNONYMS: [(&str, &str); 25] = [
    ("enter", "return"),
    ("esc", "escape"),
    ("del", "delete"),
    ("backspace", "back"),
    ("pgup", "prior"),
    ("pageup", "prior"),
    ("pgdn", "next"),
    ("pagedown", "next"),
    ("caps", "capital"),
    ("capslock", "capital"),
    ("super", "win"),
    ("windows", "win"),
    ("option", "alt"),
    ("scrollup", "wheelup"),
    ("scroll_up", "wheelup"),
    ("scrolldown", "wheeldown"),
    ("scroll_down", "wheeldown"),
    ("mouse1", "lbutton"),
    ("leftclick", "lbutton"),
    ("mouse2", "rbutton"),
    ("rightclick", "rbutton"),
    ("mouse3", "mbutton"),
    ("middleclick", "mbutton"),
    ("mouse4", "xbutton1"),
    ("mouse5", "xbutton2"),
];

/// Whether `name` is a key `win-hotkeys` accepts, or a mouse input or gesture whkd handles itself
#[must_use]
pub fn is_valid(name: &str) -> bool {
    MouseInput::parse(name).is_some()
        || GestureInput::parse(name).is_some()
        || VKey::from_keyname(name).is_ok()
}

/// Whether `name` is a modifier (shift, ctrl, alt or win, either side)
#[must_use]
pub fn is_modifier(name: &str) -> bool {
    matches!(
        VKey::from_keyname(name),
        Ok(VKey::Shift
            | VKey::LShift
            | VKey::RShift
            | VKey::Control
            | VKey::LControl
            | VKey::RControl
            | VKey::Menu
            | VKey::LMenu
            | VKey::RMenu
            | VKey::LWin
            | VKey::RWin)
    )
}

/// Every key name, lowercase, sorted
#[must_use]
pub fn names() -> Vec<String> {
    let mut names: Vec<String> = (0..=255)
        .map(|code| VKey::from_vk_code(code).to_string())
        .filter_map(|name| name.strip_prefix("VK_").map(str::to_lowercase))
        .filter(|name| is_valid(name))
        .chain(
            ALIASES
                .into_iter()
                .filter(|name| is_valid(name))
                .map(String::from),
        )
        .chain(MouseInput::ALL.map(|input| input.name().to_string()))
        .chain(GestureInput::ALL.map(|input| input.name().to_string()))
        .collect();

    names.sort();
    names.dedup();
    names
}

/// The names from [`names`] that are modifiers
#[must_use]
pub fn modifiers() -> Vec<String> {
    names()
        .into_iter()
        .filter(|name| is_modifier(name))
        .collect()
}

/// The validator `whkd` hands to the parser, so that a typo in a key name is reported where it
/// is, with a suggestion
#[must_use]
pub fn validator() -> KeyValidator {
    KeyValidator::new(is_valid, names())
        .with_synonyms(&SYNONYMS)
        .with_modifiers(is_modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_all_valid() {
        let names = names();
        assert!(names.len() > 50);
        for name in [
            "a", "h", "return", "escape", "alt", "ctrl", "shift", "win", "f11",
        ] {
            assert!(names.iter().any(|n| n == name), "missing {name}");
        }
        for name in &names {
            assert!(is_valid(name), "{name} is not valid");
        }
    }

    #[test]
    fn modifiers_are_the_modifiers() {
        let modifiers = modifiers();
        for name in ["alt", "ctrl", "shift", "win"] {
            assert!(modifiers.iter().any(|m| m == name), "missing {name}");
        }
        for name in ["h", "return", "f11", "space"] {
            assert!(
                !modifiers.iter().any(|m| m == name),
                "{name} is not a modifier"
            );
        }
    }

    #[test]
    fn synonyms_point_at_valid_keys() {
        for (synonym, valid) in SYNONYMS {
            assert!(!is_valid(synonym), "{synonym} is already valid");
            assert!(is_valid(valid), "{valid} is not valid");
        }
    }

    #[test]
    fn mouse_inputs_are_keys_but_not_modifiers() {
        for input in MouseInput::ALL {
            assert!(is_valid(input.name()), "{input}");
            assert!(is_valid(&input.name().to_uppercase()), "{input}");
            assert!(names().iter().any(|n| n == input.name()), "{input}");
            assert!(!is_modifier(input.name()), "{input}");
        }
    }

    #[test]
    fn a_misspelt_mouse_input_gets_a_suggestion() {
        let keys = validator();
        let help = |name: &str| {
            let src = format!(".shell cmd\nalt + {name} : x\n");
            whkd_parser::parse_with(&src, &keys).unwrap_err().help
        };

        assert_eq!(help("scrollup").as_deref(), Some("did you mean `wheelup`?"));
        assert_eq!(help("mouse3").as_deref(), Some("did you mean `mbutton`?"));
        // a transposition is a typo, not a synonym
        assert_eq!(help("whelup").as_deref(), Some("did you mean `wheelup`?"));
    }

    #[test]
    fn the_keys_held_with_a_mouse_input_must_be_modifiers() {
        let keys = validator();
        let parse = |hotkey: &str| {
            whkd_parser::parse_with(
                &format!(
                    ".shell cmd
{hotkey} : x
"
                ),
                &keys,
            )
        };

        for fine in [
            "alt + wheelup",
            "ctrl + shift + mbutton",
            "lwin + rctrl + xbutton1",
        ] {
            assert!(parse(fine).is_ok(), "{fine}");
        }

        for not_fine in [
            "alt + h + wheelup",
            "space + mbutton",
            "f1 + alt + xbutton2",
        ] {
            let e = parse(not_fine).unwrap_err();
            assert!(
                e.message.contains("cannot be held down"),
                "{not_fine}: {}",
                e.message
            );
        }

        // a keyboard hotkey can have any keys before the last, as ever
        assert!(parse("a + b + h").is_ok());
    }

    #[test]
    fn gestures_are_keys_but_not_modifiers() {
        for input in GestureInput::ALL {
            assert!(is_valid(input.name()), "{input}");
            assert!(is_valid(&input.name().to_uppercase()), "{input}");
            assert!(names().iter().any(|n| n == input.name()), "{input}");
            assert!(!is_modifier(input.name()), "{input}");
        }
    }

    #[test]
    fn a_misspelt_gesture_gets_a_suggestion() {
        let keys = validator();
        let help = |name: &str| {
            let src = format!(".shell cmd\n{name} : x\n");
            whkd_parser::parse_with(&src, &keys).unwrap_err().help
        };

        assert_eq!(
            help("swipe3lef").as_deref(),
            Some("did you mean `swipe3left`?")
        );
        assert_eq!(
            help("swipe4dwon").as_deref(),
            Some("did you mean `swipe4down`?")
        );
    }

    #[test]
    fn the_keys_held_with_a_gesture_must_be_modifiers() {
        let keys = validator();
        let parse =
            |hotkey: &str| whkd_parser::parse_with(&format!(".shell cmd\n{hotkey} : x\n"), &keys);

        assert!(parse("ctrl + alt + swipe3up").is_ok());
        assert!(parse("swipe4left").is_ok());
        let e = parse("space + swipe3right").unwrap_err();
        assert!(e.message.contains("cannot be held down"), "{}", e.message);
    }
}
