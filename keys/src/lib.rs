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

/// The name a key has in a whkdrc, from its virtual-key code, if it has one
#[must_use]
pub fn name_of(code: u16) -> Option<String> {
    VKey::from_vk_code(code)
        .to_string()
        .strip_prefix("VK_")
        .map(str::to_lowercase)
        .filter(|name| is_valid(name))
}

/// The modifier a virtual-key code is, on either side of the keyboard
#[must_use]
pub fn modifier_of(code: u16) -> Option<&'static str> {
    match VKey::from_vk_code(code) {
        VKey::Menu | VKey::LMenu | VKey::RMenu => Some("alt"),
        VKey::Control | VKey::LControl | VKey::RControl => Some("ctrl"),
        VKey::Shift | VKey::LShift | VKey::RShift => Some("shift"),
        VKey::LWin | VKey::RWin => Some("win"),
        _ => None,
    }
}

/// What a key did, by its virtual-key code
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum KeyEvent {
    Down(u16),
    Up(u16),
}

/// Turns the key presses of a keyboard into the hotkeys they would be written as: the modifiers
/// that are held down, and the key
#[derive(Debug, Default)]
pub struct Observer {
    held: std::collections::BTreeSet<u16>,
}

impl Observer {
    /// What to say about `event`: the hotkey if it is a key going down, `None` if the key was
    /// already down, which is what a key that is held does, or if it is a key coming up
    pub fn event(&mut self, event: KeyEvent) -> Option<String> {
        let code = match event {
            KeyEvent::Up(code) => {
                self.held.remove(&code);
                return None;
            }
            KeyEvent::Down(code) => code,
        };

        if !self.held.insert(code) {
            return None;
        }

        // a modifier that is held is not held again by the other side of the keyboard
        let pressed = modifier_of(code);
        let mut modifiers: Vec<&str> = self
            .held
            .iter()
            .filter(|held| **held != code)
            .filter_map(|held| modifier_of(*held))
            .filter(|modifier| Some(*modifier) != pressed)
            .collect();
        modifiers.sort_unstable();
        modifiers.dedup();

        let key = match pressed.map(String::from).or_else(|| name_of(code)) {
            Some(name) => name,
            None => {
                return Some(format!(
                    "(a key without a name, virtual-key code {code:#04x})"
                ))
            }
        };

        modifiers.push(&key);
        Some(modifiers.join(" + "))
    }
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

    #[test]
    fn the_keys_held_with_a_mouse_input_are_checked_in_every_expansion() {
        let keys = validator();
        let src = ".shell cmd\n{alt,h} + wheelup : x\n";
        let error = whkd_parser::parse_with(src, &keys).unwrap_err();
        assert_eq!(error.message, "`h` cannot be held down with `wheelup`");
        // at the `h` that was written, not at the text it made
        assert_eq!(&src[error.span.clone()], "h");

        let fine = whkd_parser::parse_with(
            ".shell cmd\n{alt,ctrl} + {wheelup,wheeldown} : echo {a,b}\n",
            &keys,
        );
        assert_eq!(fine.unwrap().bindings.len(), 4);
    }

    #[test]
    fn a_misspelt_key_in_a_group_gets_a_suggestion() {
        let keys = validator();
        let src = ".shell cmd\nalt + {h,retrun} : x\n";
        let error = whkd_parser::parse_with(src, &keys).unwrap_err();
        assert_eq!(&src[error.span.clone()], "retrun");
        assert_eq!(error.help.as_deref(), Some("did you mean `return`?"));
    }

    const H: u16 = 0x48;
    const LALT: u16 = 0xA4;
    const RALT: u16 = 0xA5;
    const LCTRL: u16 = 0xA2;
    const LSHIFT: u16 = 0xA0;
    const RSHIFT: u16 = 0xA1;
    const LWIN: u16 = 0x5B;

    fn press(observer: &mut Observer, codes: &[u16]) -> Vec<String> {
        codes
            .iter()
            .filter_map(|code| observer.event(KeyEvent::Down(*code)))
            .collect()
    }

    #[test]
    fn a_key_is_its_name() {
        let mut observer = Observer::default();
        assert_eq!(press(&mut observer, &[H]), ["h"]);

        for (code, name) in [
            (0x0D, "return"),
            (0x1B, "escape"),
            (0x20, "space"),
            (0x7A, "f11"),
            (0x31, "1"),
            (0x25, "left"),
        ] {
            let mut observer = Observer::default();
            assert_eq!(press(&mut observer, &[code]), [name], "{code:#x}");
        }
    }

    #[test]
    fn the_keys_that_have_to_be_looked_up_are_named_as_a_whkdrc_names_them() {
        // the ones that are not what they are on the key
        for (code, name) in [
            (0xBA, "oem_1"),
            (0xDB, "oem_4"),
            (0xDD, "oem_6"),
            (0xBB, "oem_plus"),
            (0xBE, "oem_period"),
        ] {
            let mut observer = Observer::default();
            assert_eq!(press(&mut observer, &[code]), [name], "{code:#x}");
            assert!(is_valid(name));
        }
    }

    #[test]
    fn modifiers_that_are_held_come_first_and_the_key_that_is_pressed_last() {
        let mut observer = Observer::default();
        assert_eq!(
            press(&mut observer, &[LSHIFT, LCTRL, LALT, H]),
            [
                "shift",
                "shift + ctrl",
                "ctrl + shift + alt",
                "alt + ctrl + shift + h"
            ]
        );

        let mut observer = Observer::default();
        assert_eq!(
            press(&mut observer, &[LWIN, LALT, H]),
            ["win", "win + alt", "alt + win + h"]
        );
    }

    #[test]
    fn either_side_of_the_keyboard_is_the_same_modifier() {
        let mut observer = Observer::default();
        assert_eq!(press(&mut observer, &[RALT, H]), ["alt", "alt + h"]);

        // and a modifier that is held is not held twice
        let mut observer = Observer::default();
        assert_eq!(
            press(&mut observer, &[LSHIFT, RSHIFT, H]),
            ["shift", "shift", "shift + h"]
        );
    }

    #[test]
    fn a_key_that_is_held_says_nothing_more_until_it_is_pressed_again() {
        let mut observer = Observer::default();
        assert_eq!(press(&mut observer, &[LALT, H, H, H]), ["alt", "alt + h"]);

        assert_eq!(observer.event(KeyEvent::Up(H)), None);
        assert_eq!(press(&mut observer, &[H]), ["alt + h"]);

        // a modifier that was let go of is no longer held
        assert_eq!(observer.event(KeyEvent::Up(LALT)), None);
        assert_eq!(observer.event(KeyEvent::Up(H)), None);
        assert_eq!(press(&mut observer, &[H]), ["h"]);
    }

    #[test]
    fn a_key_coming_up_that_was_never_down_is_nothing() {
        let mut observer = Observer::default();
        assert_eq!(observer.event(KeyEvent::Up(H)), None);
        assert_eq!(press(&mut observer, &[H]), ["h"]);
    }

    #[test]
    fn a_key_without_a_name_says_what_it_is() {
        let mut observer = Observer::default();
        let said = press(&mut observer, &[0xFF]);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("0xff"), "{}", said[0]);
        assert!(said[0].contains("without a name"), "{}", said[0]);
    }

    #[test]
    fn every_name_said_is_a_key_a_whkdrc_accepts() {
        for code in 0..=255u16 {
            if let Some(name) = name_of(code) {
                assert!(is_valid(&name), "{code:#x}: {name}");

                let mut observer = Observer::default();
                let said = observer.event(KeyEvent::Down(code)).unwrap();
                let valid = whkd_parser::parse_with(
                    &format!(
                        ".shell cmd
{said} : x
"
                    ),
                    &validator(),
                );
                assert!(valid.is_ok(), "{code:#x}: {said}: {valid:?}");
            }
        }
    }

    #[test]
    fn modifiers_are_told_apart_by_name() {
        for (code, modifier) in [
            (LALT, "alt"),
            (RALT, "alt"),
            (0x12, "alt"),
            (LCTRL, "ctrl"),
            (0x11, "ctrl"),
            (LSHIFT, "shift"),
            (0x10, "shift"),
            (LWIN, "win"),
            (0x5C, "win"),
        ] {
            assert_eq!(modifier_of(code), Some(modifier), "{code:#x}");
        }

        assert_eq!(modifier_of(H), None);
        assert_eq!(modifier_of(0x0D), None);
    }
}
