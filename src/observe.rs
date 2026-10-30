//! `whkd observe`: say which key each key press is, as it is written in a whkdrc.
//!
//! Finding the name of a key (`oem_1` is `;` on some keyboards and something else on others) is
//! guesswork otherwise. The hook only looks: every key still goes where it was going, and it can be
//! used while whkd is running. What a key is called, and how the modifiers that are held with it are
//! written, is in `whkd_keys::Observer`, which has no Win32 in it so that it can be tested.

use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use whkd_keys::KeyEvent;
use whkd_keys::Observer;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowsHookExW;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::HC_ACTION;
use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;
use windows::Win32::UI::WindowsAndMessaging::LLKHF_INJECTED;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYUP;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYUP;

struct Shared {
    events: mpsc::Sender<KeyEvent>,
    /// Let through what other programs synthesise, which is for tests only
    allow_injected: bool,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// What a message that the hook is given is, if it is a key going down or coming up
fn decode(message: u32, code: u32) -> Option<KeyEvent> {
    let code = u16::try_from(code).ok()?;

    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyEvent::Down(code)),
        WM_KEYUP | WM_SYSKEYUP => Some(KeyEvent::Up(code)),
        _ => None,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(shared) = SHARED.get() {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let injected = info.flags & LLKHF_INJECTED != Default::default();

            if shared.allow_injected || !injected {
                if let Some(event) = decode(wparam.0 as u32, info.vkCode) {
                    let _ = shared.events.send(event);
                }
            }
        }
    }

    // never swallow anything: this only watches
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Listen to the keyboard, and hand over what it does. Meant to be started once
pub fn start(allow_injected: bool) -> Result<mpsc::Receiver<KeyEvent>> {
    let (events, received) = mpsc::channel();

    SHARED
        .set(Shared {
            events,
            allow_injected,
        })
        .map_err(|_| eyre!("the keyboard is already being observed"))?;

    // a low-level hook is called on the thread that installed it, which has to pump messages
    thread::spawn(|| unsafe {
        let _hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) {
            Ok(hook) => hook,
            Err(error) => {
                eprintln!("Unable to listen to the keyboard (error: {error})");
                std::process::exit(1);
            }
        };

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });

    Ok(received)
}

/// Write the hotkey of each key that is pressed, until the program is stopped
pub fn run() -> Result<()> {
    let events = start(false)?;

    eprintln!(
        "Press keys, and what to write in a whkdrc for each is written here. Press Ctrl+C to stop."
    );

    let mut observer = Observer::default();
    for event in events {
        if let Some(hotkey) = observer.event(event) {
            println!("{hotkey}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn keys_going_down_and_up_are_events() {
        assert_eq!(decode(WM_KEYDOWN, 0x48), Some(KeyEvent::Down(0x48)));
        // a key pressed with alt held is a system key
        assert_eq!(decode(WM_SYSKEYDOWN, 0x48), Some(KeyEvent::Down(0x48)));
        assert_eq!(decode(WM_KEYUP, 0x48), Some(KeyEvent::Up(0x48)));
        assert_eq!(decode(WM_SYSKEYUP, 0x48), Some(KeyEvent::Up(0x48)));
    }

    #[test]
    fn other_messages_and_codes_are_not_events() {
        assert_eq!(decode(0, 0x48), None);
        assert_eq!(decode(WM_KEYDOWN, 0x1_0000), None);
    }

    /// Starts a real hook and sends real, if synthetic, key presses, so it is not run by default:
    /// `cargo test -p whkd -- --ignored observe`. Nothing else should be using the keyboard meanwhile.
    #[test]
    #[ignore = "starts a real keyboard hook and sends synthetic key presses"]
    fn synthetic_key_presses_are_observed() {
        use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
        use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

        fn send(code: u16, flags: KEYBD_EVENT_FLAGS) {
            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(code),
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        }

        let events = start(true).unwrap();

        // the hook is installed on its own thread, so give it a moment
        thread::sleep(Duration::from_millis(500));

        // F13 and F14 are keys that nothing is bound to
        send(0x7C, KEYBD_EVENT_FLAGS(0));
        send(0x7C, KEYEVENTF_KEYUP);
        send(0x7D, KEYBD_EVENT_FLAGS(0));
        send(0x7D, KEYEVENTF_KEYUP);

        let mut observer = Observer::default();
        let mut said = vec![];
        while said.len() < 2 {
            let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
            said.extend(observer.event(event));
        }

        assert_eq!(said, ["f13", "f14"]);
    }
}
