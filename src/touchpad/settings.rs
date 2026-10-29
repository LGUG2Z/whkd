//! Windows acts on three and four finger gestures itself, and whkd cannot stop it: the touchpad's
//! raw input is only watched, never taken. So a swipe that is bound in the whkdrc also does what
//! Windows does with it, unless that is turned off in Settings (Bluetooth & devices > Touchpad).
//! `whkd validate` says so, from what is in the registry.
//!
//! The registry values come from write-ups of the Settings page, not from Microsoft's
//! documentation, so what is said is advice, and nothing here ever writes to the registry.

use std::collections::BTreeSet;
use std::ffi::c_void;
use whkd_core::GestureInput;
use whkd_core::Whkdrc;
use windows::core::w;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::RegGetValueW;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::Registry::RRF_RT_REG_DWORD;

/// How many fingers the gestures bound in a whkdrc use
#[must_use]
pub fn fingers_bound(whkdrc: &Whkdrc) -> BTreeSet<u8> {
    let bound = whkdrc.bindings.iter().chain(
        whkdrc
            .app_bindings
            .iter()
            .flat_map(|(_, bindings)| bindings),
    );

    bound
        .filter_map(|binding| binding.keys.last())
        .filter_map(|key| GestureInput::parse(key))
        .map(GestureInput::fingers)
        .collect()
}

/// Whether Windows will act on a swipe of this many fingers, given what the registry says. When the
/// setting has never been changed it is not there, and Windows does what it does by default
#[must_use]
pub fn windows_will_act(value: Option<u32>) -> bool {
    value != Some(0)
}

/// The registry value for a swipe of this many fingers: 0 is "Nothing"
fn read_swipe_setting(fingers: u8) -> Option<u32> {
    let name = match fingers {
        3 => w!("ThreeFingerSlideEnabled"),
        4 => w!("FourFingerSlideEnabled"),
        _ => return None,
    };

    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast::<c_void>()),
            Some(&mut size),
        )
    };

    (status == ERROR_SUCCESS).then_some(value)
}

fn word(fingers: u8) -> &'static str {
    if fingers == 3 {
        "three"
    } else {
        "four"
    }
}

/// What to tell someone whose whkdrc binds swipes, if Windows is going to act on them too
#[must_use]
pub fn notes(whkdrc: &Whkdrc) -> Vec<String> {
    notes_with(whkdrc, read_swipe_setting)
}

fn notes_with(whkdrc: &Whkdrc, setting: impl Fn(u8) -> Option<u32>) -> Vec<String> {
    fingers_bound(whkdrc)
        .into_iter()
        .filter(|fingers| windows_will_act(setting(*fingers)))
        .map(|fingers| {
            let word = word(fingers);
            format!(
                "note: Windows will also act on {word}-finger swipes, as well as the bindings for them. To stop it, set Settings > Bluetooth & devices > Touchpad > {} gestures > Swipes to Nothing",
                word[..1].to_uppercase() + &word[1..] + "-finger"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use whkd_core::Condition;
    use whkd_core::HotkeyBinding;
    use whkd_core::Shell;

    fn binding(keys: &[&str]) -> HotkeyBinding {
        HotkeyBinding {
            keys: keys.iter().map(ToString::to_string).collect(),
            command: String::from("x"),
            condition: Condition::Always,
        }
    }

    fn whkdrc(plain: &[&[&str]], conditional: &[&[&str]]) -> Whkdrc {
        Whkdrc {
            shell: Shell::Cmd,
            app_bindings: conditional
                .iter()
                .map(|keys| {
                    (
                        keys.iter().map(ToString::to_string).collect(),
                        vec![binding(keys)],
                    )
                })
                .collect(),
            bindings: plain.iter().map(|keys| binding(keys)).collect(),
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
        }
    }

    #[test]
    fn the_fingers_of_the_bound_gestures_are_found_wherever_they_are() {
        let config = whkdrc(
            &[&["alt", "h"], &["swipe3left"], &["alt", "Swipe3"]],
            &[&["swipe4up"]],
        );
        assert_eq!(fingers_bound(&config), BTreeSet::from([3, 4]));

        let config = whkdrc(&[&["alt", "h"], &["alt", "wheelup"]], &[]);
        assert!(fingers_bound(&config).is_empty());

        // a gesture's name is the last key
        let config = whkdrc(&[&["swipe3left", "alt"]], &[]);
        assert!(fingers_bound(&config).is_empty());
    }

    #[test]
    fn windows_acts_unless_the_setting_is_nothing() {
        // never changed: not in the registry, and Windows does what it does by default
        assert!(windows_will_act(None));
        assert!(windows_will_act(Some(1)));
        assert!(windows_will_act(Some(2)));
        assert!(windows_will_act(Some(3)));
        assert!(!windows_will_act(Some(0)));
    }

    #[test]
    fn only_the_gestures_that_are_bound_are_mentioned() {
        let config = whkdrc(&[&["swipe3left"]], &[]);
        let notes = notes_with(&config, |_| None);

        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("three-finger swipes"), "{}", notes[0]);
        assert!(notes[0].contains("Three-finger gestures"), "{}", notes[0]);
        assert!(notes[0].contains("Swipes to Nothing"));
    }

    #[test]
    fn nothing_is_said_when_windows_has_been_told_to_do_nothing() {
        let config = whkdrc(&[&["swipe3left"], &["swipe4right"]], &[]);

        assert!(notes_with(&config, |_| Some(0)).is_empty());

        // three is turned off and four is not
        let notes = notes_with(&config, |fingers| (fingers == 3).then_some(0));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("four-finger"));
    }

    #[test]
    fn nothing_is_said_about_a_config_without_gestures() {
        let config = whkdrc(&[&["alt", "h"]], &[]);
        assert!(notes_with(&config, |_| None).is_empty());
    }

    /// Says what this machine's registry holds, and so what `whkd validate` would say here. Reads the
    /// registry, so it is not run by default
    #[test]
    #[ignore = "reads the registry of the machine it runs on"]
    fn the_settings_here() {
        for fingers in [3, 4] {
            let value = read_swipe_setting(fingers);
            println!(
                "{fingers} fingers: {value:?}, Windows will act: {}",
                windows_will_act(value)
            );
        }
    }
}
