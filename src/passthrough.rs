//! Letting keys through to the applications that `.ignore` lists.
//!
//! The keyboard hook that runs the hotkeys swallows a key as soon as it has a binding, and decides
//! that before it is known what the binding would run. What it does not do while it is paused is
//! swallow anything, so the way to leave an application alone is to be paused while it has focus.
//! The mouse and the touchpad are paused by the same handle, so they are left alone as well.
//!
//! Pausing is also something that a person does, with the `.pause` hotkey, and that is theirs: whkd
//! only resumes what it paused itself. Which of the two to do is [`AutoPause`], which has no Win32 in
//! it so that it can be tested. [`start`] is what notices which application has focus.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use whkd_core::is_ignored;
use win_hotkeys::PauseHandle;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::EVENT_SYSTEM_FOREGROUND;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::WINEVENT_OUTOFCONTEXT;

/// What to do about the pause when the focus is somewhere
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Action {
    Pause,
    Resume,
    Nothing,
}

/// Remembers whether whkd is paused because an ignored application has focus, which is the only
/// pause it takes back
#[derive(Debug, Default)]
pub struct AutoPause {
    paused_by_us: bool,
}

impl AutoPause {
    /// What to do now that the focused application is, or is not, an ignored one, and whkd is, or is
    /// not, paused
    pub fn update(&mut self, ignored: bool, paused: bool) -> Action {
        match (ignored, paused) {
            (true, false) => {
                self.paused_by_us = true;
                Action::Pause
            }
            // paused already, by a person or by us: either way, it is staying that way
            (true, true) => Action::Nothing,
            (false, _) if !self.paused_by_us => Action::Nothing,
            (false, paused) => {
                self.paused_by_us = false;
                // a person who has unpaused in the meantime has done it already
                if paused {
                    Action::Resume
                } else {
                    Action::Nothing
                }
            }
        }
    }
}

/// Pause or resume `pause` for the application that now has focus
pub fn apply(state: &mut AutoPause, ignore: &[String], app: &str, pause: &PauseHandle) -> Action {
    let action = state.update(is_ignored(ignore, app), pause.is_paused());

    match action {
        Action::Pause => pause.set(true),
        Action::Resume => pause.set(false),
        Action::Nothing => {}
    }

    action
}

struct Shared {
    ignore: Vec<String>,
    pause: PauseHandle,
    focused: mpsc::Sender<()>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if let Some(shared) = SHARED.get() {
        let _ = shared.focused.send(());
    }
}

/// The application with the focused window
fn focused_app() -> String {
    active_win_pos_rs::get_active_window()
        .unwrap_or_default()
        .app_name
}

/// From now on, keep `pause` paused while an application in `ignore` has focus. Meant to be started
/// once, and only if there is something to ignore
pub fn start(ignore: Vec<String>, pause: PauseHandle) {
    let (focused, changes) = mpsc::channel();

    if SHARED
        .set(Shared {
            ignore,
            pause,
            focused,
        })
        .is_err()
    {
        return;
    }

    // what the window is called is looked up off the thread that Windows calls, which has to be
    // quick, and gets to the focus before any key that is pressed after it
    thread::spawn(move || {
        let Some(shared) = SHARED.get() else { return };
        let mut state = AutoPause::default();

        // the window that has focus now, which nothing will say
        apply(&mut state, &shared.ignore, &focused_app(), &shared.pause);

        for () in changes {
            apply(&mut state, &shared.ignore, &focused_app(), &shared.pause);
        }
    });

    // an event hook is called on the thread that installed it, which has to pump messages
    thread::spawn(|| unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        if hook.is_invalid() {
            eprintln!(
                "Unable to watch which application has focus, so keys will not be let through to the applications in `.ignore`"
            );
            return;
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use win_hotkeys::HotkeyManager;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn an_ignored_application_pauses_and_leaving_it_resumes() {
        let mut state = AutoPause::default();

        assert_eq!(state.update(true, false), Action::Pause);
        // still there, and still paused
        assert_eq!(state.update(true, true), Action::Nothing);
        assert_eq!(state.update(false, true), Action::Resume);
        assert_eq!(state.update(false, false), Action::Nothing);
    }

    #[test]
    fn other_applications_leave_whkd_as_it_is() {
        let mut state = AutoPause::default();

        assert_eq!(state.update(false, false), Action::Nothing);
        assert_eq!(state.update(false, false), Action::Nothing);
    }

    #[test]
    fn a_pause_that_a_person_took_is_not_taken_back() {
        let mut state = AutoPause::default();

        // paused with the hotkey, and then focus goes to an ignored application and away again
        assert_eq!(state.update(true, true), Action::Nothing);
        assert_eq!(state.update(false, true), Action::Nothing);

        // and still, when it has been through several
        assert_eq!(state.update(true, true), Action::Nothing);
        assert_eq!(state.update(true, true), Action::Nothing);
        assert_eq!(state.update(false, true), Action::Nothing);
    }

    #[test]
    fn a_person_who_unpauses_in_an_ignored_application_is_left_unpaused_on_leaving_it() {
        let mut state = AutoPause::default();

        assert_eq!(state.update(true, false), Action::Pause);
        // the hotkey, pressed there
        assert_eq!(state.update(false, false), Action::Nothing);
        // and it is not something to resume again
        assert_eq!(state.update(false, false), Action::Nothing);
    }

    #[test]
    fn a_person_who_unpauses_in_an_ignored_application_has_it_paused_again_on_the_next_focus_there()
    {
        let mut state = AutoPause::default();

        assert_eq!(state.update(true, false), Action::Pause);
        // unpaused with the hotkey, then focus moves between two ignored applications
        assert_eq!(state.update(true, false), Action::Pause);
        assert_eq!(state.update(false, true), Action::Resume);
    }

    #[test]
    fn the_pause_handle_follows_the_focus() {
        let pause = HotkeyManager::<()>::new().pause_handle();
        let ignore = names(&["Firefox", "Google Chrome"]);
        let mut state = AutoPause::default();

        assert!(!pause.is_paused());

        assert_eq!(apply(&mut state, &ignore, "Code", &pause), Action::Nothing);
        assert!(!pause.is_paused());

        assert_eq!(apply(&mut state, &ignore, "Firefox", &pause), Action::Pause);
        assert!(pause.is_paused());

        assert_eq!(
            apply(&mut state, &ignore, "Google Chrome", &pause),
            Action::Nothing
        );
        assert!(pause.is_paused());

        assert_eq!(apply(&mut state, &ignore, "Code", &pause), Action::Resume);
        assert!(!pause.is_paused());
    }

    #[test]
    fn names_have_to_be_exact() {
        let pause = HotkeyManager::<()>::new().pause_handle();
        let ignore = names(&["Firefox"]);
        let mut state = AutoPause::default();

        for app in ["firefox", "Firefox Developer Edition", ""] {
            assert_eq!(apply(&mut state, &ignore, app, &pause), Action::Nothing);
            assert!(!pause.is_paused(), "{app:?}");
        }
    }

    #[test]
    fn a_manual_pause_survives_a_visit_to_an_ignored_application() {
        let pause = HotkeyManager::<()>::new().pause_handle();
        let ignore = names(&["Firefox"]);
        let mut state = AutoPause::default();

        // `.pause` was pressed
        pause.set(true);

        assert_eq!(
            apply(&mut state, &ignore, "Firefox", &pause),
            Action::Nothing
        );
        assert_eq!(apply(&mut state, &ignore, "Code", &pause), Action::Nothing);
        assert!(pause.is_paused());
    }

    /// Keys that a hotkey is bound to get through to what is below the hook while it is paused, and
    /// not while it is not. Starts real hooks and sends synthetic key presses, so it is not run by
    /// default: `cargo test -p whkd -- --ignored passthrough`.
    #[test]
    #[ignore = "starts real keyboard hooks and sends synthetic key presses"]
    fn a_key_goes_through_while_an_ignored_application_has_focus() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        use std::time::Duration;
        use whkd_keys::KeyEvent;
        use win_hotkeys::VKey;
        use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
        use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
        use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

        const F24: u16 = 0x87;

        fn tap(code: u16) {
            for flags in [KEYBD_EVENT_FLAGS(0), KEYEVENTF_KEYUP] {
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
        }

        // what watches is started first: a hook that is installed later is called earlier, and can
        // swallow the key before it gets there
        let events = crate::observe::start(true).unwrap();
        thread::sleep(Duration::from_millis(500));

        let fired = Arc::new(AtomicUsize::new(0));
        let (handle, handles) = mpsc::channel();
        {
            let fired = fired.clone();
            thread::spawn(move || {
                let mut hkm = HotkeyManager::<()>::new();
                hkm.register_hotkey(VKey::F24, &[], move || {
                    fired.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
                handle.send(hkm.pause_handle()).unwrap();
                hkm.event_loop();
            });
        }
        let pause = handles.recv().unwrap();
        thread::sleep(Duration::from_millis(500));

        let reached = |events: &mpsc::Receiver<KeyEvent>| {
            let mut down = 0;
            while let Ok(event) = events.recv_timeout(Duration::from_millis(600)) {
                if event == KeyEvent::Down(F24) {
                    down += 1;
                }
            }
            down
        };

        let ignore = names(&["Firefox"]);
        let mut state = AutoPause::default();

        // in an application that is not ignored: the hotkey runs, and the key is consumed
        apply(&mut state, &ignore, "Code", &pause);
        tap(F24);
        assert_eq!(reached(&events), 0);
        assert_eq!(fired.load(Ordering::SeqCst), 1);

        // in one that is: nothing runs, and the key goes where it was going
        apply(&mut state, &ignore, "Firefox", &pause);
        tap(F24);
        assert_eq!(reached(&events), 1);
        assert_eq!(fired.load(Ordering::SeqCst), 1);

        // and back
        apply(&mut state, &ignore, "Code", &pause);
        tap(F24);
        assert_eq!(reached(&events), 0);
        assert_eq!(fired.load(Ordering::SeqCst), 2);
    }

    /// Watches the real focus: the application that has focus when it starts is ignored, so whkd
    /// has to end up paused, and a different list leaves it alone. Needs a desktop with a window
    /// that has focus, so it is not run by default: `cargo test -p whkd -- --ignored passthrough`.
    #[test]
    #[ignore = "needs a desktop with a focused window"]
    fn the_focused_application_is_looked_up_and_ignored() {
        use std::time::Duration;
        use std::time::Instant;

        let app = focused_app();
        if app.is_empty() {
            return;
        }

        let pause = HotkeyManager::<()>::new().pause_handle();
        start(
            vec![String::from("Not An Application"), app.clone()],
            pause.clone(),
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        while !pause.is_paused() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }

        assert!(pause.is_paused(), "{app} has focus and is ignored");
    }
}
