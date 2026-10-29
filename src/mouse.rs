//! Mouse bindings: the pure logic.
//!
//! Which bindings apply to a mouse event, when a press or a wheel notch fires a command, and what to
//! swallow, with no Win32 in it so that it can be tested. The mouse hook feeds this the events it
//! sees and does what it says.
//!
//! A hook has to answer at once (Windows removes a hook that is slow), so what is swallowed is
//! decided from the bindings alone. Which command a binding runs, which can depend on the focused
//! application or on komorebi, is decided later, off the hook's thread.

use crate::HkmData;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use whkd_core::MouseInput;
use win_hotkeys::state::KeyboardState;
use win_hotkeys::PauseHandle;
use win_hotkeys::VKey;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowsHookExW;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::HC_ACTION;
use windows::Win32::UI::WindowsAndMessaging::LLMHF_INJECTED;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::MSLLHOOKSTRUCT;
use windows::Win32::UI::WindowsAndMessaging::WH_MOUSE_LL;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEHWHEEL;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_XBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_XBUTTONUP;

/// One notch of a mouse wheel, in the units Windows reports. Smooth scrolling (a trackpad, or a
/// high resolution wheel) reports fractions of it, so deltas are accumulated until they add up
const WHEEL_DELTA: i32 = 120;

/// Scrolling that stops for this long starts counting from nothing again
const WHEEL_IDLE: Duration = Duration::from_millis(500);

/// The keyboard modifiers that count when a mouse input is used, either side of the keyboard
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const ALT: Self = Self(1);
    pub const CTRL: Self = Self(1 << 1);
    pub const SHIFT: Self = Self(1 << 2);
    pub const WIN: Self = Self(1 << 3);

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The modifier a key is, if it is one
    pub fn of(key: VKey) -> Option<Self> {
        match key {
            VKey::Menu | VKey::LMenu | VKey::RMenu => Some(Self::ALT),
            VKey::Control | VKey::LControl | VKey::RControl => Some(Self::CTRL),
            VKey::Shift | VKey::LShift | VKey::RShift => Some(Self::SHIFT),
            VKey::LWin | VKey::RWin => Some(Self::WIN),
            _ => None,
        }
    }

    /// The modifiers among `keys`. Keys that are not modifiers (`a + b + wheelup`) are not held
    /// down in any way a mouse hook can see, so a binding that uses one is reported by the caller
    pub fn from_keys(keys: &[VKey]) -> Self {
        keys.iter()
            .filter_map(|key| Self::of(*key))
            .fold(Self::NONE, Self::union)
    }

    /// The modifiers currently held, given a way to ask whether a key is down
    pub fn from_pressed(is_down: impl Fn(VKey) -> bool) -> Self {
        [
            (Self::ALT, [VKey::LMenu, VKey::RMenu]),
            (Self::CTRL, [VKey::LControl, VKey::RControl]),
            (Self::SHIFT, [VKey::LShift, VKey::RShift]),
            (Self::WIN, [VKey::LWin, VKey::RWin]),
        ]
        .into_iter()
        .filter(|(_, keys)| keys.iter().any(|key| is_down(*key)))
        .fold(Self::NONE, |all, (modifier, _)| all.union(modifier))
    }
}

/// What the hook saw
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Event {
    ButtonDown(MouseInput),
    ButtonUp(MouseInput),
    /// A scroll, with the delta Windows reported. Positive is away from the user (up) or to the
    /// right
    Wheel {
        horizontal: bool,
        delta: i32,
    },
}

/// What the hook should do
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<T> {
    /// Not ours: let the event through
    Pass,
    /// Ours, but nothing to run yet (a release, or part of a notch): hide it from the application
    Swallow,
    /// Ours: hide it from the application and run the binding
    Fire(T),
}

#[derive(Debug, Default)]
struct Wheel {
    accumulated: i32,
    last: Option<Instant>,
    /// When a binding last fired, and whether that was for positive deltas (up, or right)
    fired: Option<(Instant, bool)>,
}

/// The bindings, and the little state that deciding about a stream of events needs
#[derive(Debug)]
pub struct Handler<T> {
    bindings: HashMap<(MouseInput, Modifiers), T>,
    /// Buttons whose press was swallowed, so that the release is too and the application never sees
    /// a release without a press
    swallowed: HashSet<MouseInput>,
    /// How long a wheel binding ignores scrolling in the direction it fired for, once it has
    /// fired
    cooldown: Duration,
    vertical: Wheel,
    horizontal: Wheel,
}

impl<T: Clone> Handler<T> {
    pub fn new(
        bindings: impl IntoIterator<Item = ((MouseInput, Modifiers), T)>,
        cooldown: Duration,
    ) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            swallowed: HashSet::new(),
            cooldown,
            vertical: Wheel::default(),
            horizontal: Wheel::default(),
        }
    }

    /// Decide what to do with `event`, with `modifiers` held and the time `now`
    pub fn handle(&mut self, event: Event, modifiers: Modifiers, now: Instant) -> Decision<T> {
        match event {
            Event::ButtonDown(input) => match self.bindings.get(&(input, modifiers)) {
                Some(binding) => {
                    self.swallowed.insert(input);
                    Decision::Fire(binding.clone())
                }
                None => Decision::Pass,
            },
            // whatever is held now: what matters is that the press was swallowed
            Event::ButtonUp(input) => {
                if self.swallowed.remove(&input) {
                    Decision::Swallow
                } else {
                    Decision::Pass
                }
            }
            Event::Wheel { horizontal, delta } => self.wheel(horizontal, delta, modifiers, now),
        }
    }

    fn wheel(
        &mut self,
        horizontal: bool,
        delta: i32,
        modifiers: Modifiers,
        now: Instant,
    ) -> Decision<T> {
        let input = match (horizontal, delta > 0) {
            (false, true) => MouseInput::WheelUp,
            (false, false) => MouseInput::WheelDown,
            (true, true) => MouseInput::WheelRight,
            (true, false) => MouseInput::WheelLeft,
        };

        let cooldown = self.cooldown;
        let positive = delta > 0;

        let wheel = if horizontal {
            &mut self.horizontal
        } else {
            &mut self.vertical
        };

        let Some(binding) = self.bindings.get(&(input, modifiers)) else {
            // not bound, so scrolling this way is not ours and starts counting again
            *wheel = Wheel::default();
            return Decision::Pass;
        };

        // just after a firing, scrolling the same way is swallowed and forgotten, so that a trackpad
        // that carries on scrolling after the fingers lift does not fire the binding again and
        // again. The cooldown counts from the firing, not from the last event, or momentum that
        // never stopped would never let the binding fire again
        if let Some((fired_at, fired_positive)) = wheel.fired {
            if fired_positive == positive && now.saturating_duration_since(fired_at) < cooldown {
                wheel.accumulated = 0;
                wheel.last = Some(now);
                return Decision::Swallow;
            }
        }

        let idle = wheel
            .last
            .is_none_or(|last| now.saturating_duration_since(last) > WHEEL_IDLE);
        let reversed = wheel.accumulated.signum() * delta.signum() < 0;
        if idle || reversed {
            wheel.accumulated = 0;
        }

        wheel.last = Some(now);
        wheel.accumulated += delta;

        // one firing per notch, and the rest is kept for the next event
        if wheel.accumulated.abs() >= WHEEL_DELTA {
            wheel.accumulated -= WHEEL_DELTA * wheel.accumulated.signum();
            wheel.fired = Some((now, positive));
            Decision::Fire(binding.clone())
        } else {
            Decision::Swallow
        }
    }
}

/// The bindings that apply to one mouse input with one set of modifiers, in file order. A binding
/// without conditions comes last, as what runs when none of the others match
pub type Entries = Arc<Vec<HkmData>>;

struct Shared {
    handler: Mutex<Handler<Entries>>,
    /// To the worker thread, which works out the command and runs it. The hook must not do that: it
    /// can involve asking komorebi a question, and a hook that is slow is removed by Windows
    fire: mpsc::Sender<Entries>,
    paused: PauseHandle,
    allow_injected: bool,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// What a message from the mouse hook means. `mouse_data` is the `mouseData` of the hook's
/// `MSLLHOOKSTRUCT`: for a wheel its high word is the (signed) delta, for an extra button its high
/// word says which one
#[must_use]
pub fn decode(message: u32, mouse_data: u32) -> Option<Event> {
    let high_word = (mouse_data >> 16) as u16;

    let extra_button = || match high_word {
        1 => Some(MouseInput::XButton1),
        2 => Some(MouseInput::XButton2),
        _ => None,
    };

    match message {
        WM_LBUTTONDOWN => Some(Event::ButtonDown(MouseInput::LButton)),
        WM_LBUTTONUP => Some(Event::ButtonUp(MouseInput::LButton)),
        WM_RBUTTONDOWN => Some(Event::ButtonDown(MouseInput::RButton)),
        WM_RBUTTONUP => Some(Event::ButtonUp(MouseInput::RButton)),
        WM_MBUTTONDOWN => Some(Event::ButtonDown(MouseInput::MButton)),
        WM_MBUTTONUP => Some(Event::ButtonUp(MouseInput::MButton)),
        WM_XBUTTONDOWN => extra_button().map(Event::ButtonDown),
        WM_XBUTTONUP => extra_button().map(Event::ButtonUp),
        WM_MOUSEWHEEL => Some(Event::Wheel {
            horizontal: false,
            delta: i32::from(high_word as i16),
        }),
        WM_MOUSEHWHEEL => Some(Event::Wheel {
            horizontal: true,
            delta: i32::from(high_word as i16),
        }),
        _ => None,
    }
}

/// Whether the event is ours, and so is to be hidden from the application
fn swallow(message: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let Some(shared) = SHARED.get() else {
        return false;
    };

    let Some(event) = decode(message, info.mouseData) else {
        return false;
    };

    // input that was synthesised, perhaps by a command whkd ran, is never ours: that could loop
    if info.flags & LLMHF_INJECTED != 0 && !shared.allow_injected {
        return false;
    }

    // while paused nothing new fires, but a release still has to be swallowed if its press was, or
    // the application would see a release without a press
    if shared.paused.is_paused() && !matches!(event, Event::ButtonUp(_)) {
        return false;
    }

    let modifiers =
        Modifiers::from_pressed(|key| KeyboardState::get_async_key_state(key.to_vk_code()));

    match shared
        .handler
        .lock()
        .handle(event, modifiers, Instant::now())
    {
        Decision::Pass => false,
        Decision::Swallow => true,
        Decision::Fire(entries) => {
            let _ = shared.fire.send(entries);
            true
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // the mouse moves a lot, and none of it is ours
    if code == HC_ACTION as i32 && wparam.0 as u32 != WM_MOUSEMOVE {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if swallow(wparam.0 as u32, info) {
            return LRESULT(1);
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Listen for mouse input, and call `on_fire` with the entries of each binding that fires, on a
/// worker thread. Meant to be started once, and only if there are mouse bindings: the hook sees
/// every mouse event for as long as it runs.
///
/// `allow_injected` lets synthesised input through to the bindings, which is for tests only.
pub fn start(
    bindings: impl IntoIterator<Item = ((MouseInput, Modifiers), Entries)>,
    paused: PauseHandle,
    cooldown: Duration,
    allow_injected: bool,
    on_fire: impl Fn(Entries) + Send + 'static,
) -> Result<()> {
    let (fire, queue) = mpsc::channel::<Entries>();

    SHARED
        .set(Shared {
            handler: Mutex::new(Handler::new(bindings, cooldown)),
            fire,
            paused,
            allow_injected,
        })
        .map_err(|_| eyre!("the mouse hook is already running"))?;

    thread::spawn(move || {
        for entries in queue {
            on_fire(entries);
        }
    });

    // a low-level hook is called on the thread that installed it, which has to pump messages
    thread::spawn(|| unsafe {
        let _hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0) {
            Ok(hook) => hook,
            Err(error) => {
                eprintln!(
                    "Unable to listen for mouse input (error: {error}), mouse bindings will not work"
                );
                return;
            }
        };

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT: Modifiers = Modifiers::ALT;

    fn handler() -> Handler<&'static str> {
        Handler::new(
            [
                ((MouseInput::WheelUp, ALT), "up"),
                ((MouseInput::WheelDown, ALT), "down"),
                ((MouseInput::WheelRight, ALT), "right"),
                ((MouseInput::MButton, ALT), "middle"),
                (
                    (MouseInput::LButton, ALT.union(Modifiers::SHIFT)),
                    "shift-left",
                ),
                ((MouseInput::XButton1, Modifiers::NONE), "back"),
            ],
            Duration::ZERO,
        )
    }

    fn wheel(delta: i32) -> Event {
        Event::Wheel {
            horizontal: false,
            delta,
        }
    }

    #[test]
    fn modifiers_are_the_same_on_either_side() {
        assert_eq!(Modifiers::of(VKey::LMenu), Some(Modifiers::ALT));
        assert_eq!(Modifiers::of(VKey::RMenu), Some(Modifiers::ALT));
        assert_eq!(Modifiers::of(VKey::Menu), Some(Modifiers::ALT));
        assert_eq!(Modifiers::of(VKey::RControl), Some(Modifiers::CTRL));
        assert_eq!(Modifiers::of(VKey::LShift), Some(Modifiers::SHIFT));
        assert_eq!(Modifiers::of(VKey::RWin), Some(Modifiers::WIN));
        assert_eq!(Modifiers::of(VKey::H), None);
        assert_eq!(Modifiers::of(VKey::Space), None);
    }

    #[test]
    fn modifiers_combine_and_ignore_repeats() {
        let keys = [VKey::LMenu, VKey::RMenu, VKey::Shift];
        assert_eq!(Modifiers::from_keys(&keys), ALT.union(Modifiers::SHIFT));
        assert_eq!(Modifiers::from_keys(&[]), Modifiers::NONE);
        // a key that is not a modifier adds nothing
        assert_eq!(
            Modifiers::from_keys(&[VKey::A, VKey::Control]),
            Modifiers::CTRL
        );
    }

    #[test]
    fn held_modifiers_are_read_from_the_keys_that_are_down() {
        let held = |down: &'static [VKey]| Modifiers::from_pressed(move |key| down.contains(&key));

        assert_eq!(held(&[]), Modifiers::NONE);
        assert_eq!(held(&[VKey::RMenu]), ALT);
        assert_eq!(held(&[VKey::LMenu, VKey::RMenu]), ALT);
        assert_eq!(
            held(&[VKey::LControl, VKey::LShift, VKey::LWin]),
            Modifiers::CTRL
                .union(Modifiers::SHIFT)
                .union(Modifiers::WIN)
        );
        // other keys do not count
        assert_eq!(held(&[VKey::A, VKey::Space]), Modifiers::NONE);
    }

    #[test]
    fn a_bound_press_fires_and_its_release_is_swallowed() {
        let mut h = handler();
        let now = Instant::now();

        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::MButton), ALT, now),
            Decision::Fire("middle")
        );
        assert_eq!(
            h.handle(Event::ButtonUp(MouseInput::MButton), ALT, now),
            Decision::Swallow
        );
        // only once: a second release is not ours
        assert_eq!(
            h.handle(Event::ButtonUp(MouseInput::MButton), ALT, now),
            Decision::Pass
        );
    }

    #[test]
    fn the_release_is_swallowed_even_if_the_modifier_was_let_go_first() {
        let mut h = handler();
        let now = Instant::now();

        h.handle(Event::ButtonDown(MouseInput::MButton), ALT, now);
        assert_eq!(
            h.handle(Event::ButtonUp(MouseInput::MButton), Modifiers::NONE, now),
            Decision::Swallow
        );
    }

    #[test]
    fn a_press_that_is_not_bound_is_not_ours_and_neither_is_its_release() {
        let mut h = handler();
        let now = Instant::now();

        // the same button without the modifier, and with an extra one
        for modifiers in [Modifiers::NONE, ALT.union(Modifiers::CTRL)] {
            assert_eq!(
                h.handle(Event::ButtonDown(MouseInput::MButton), modifiers, now),
                Decision::Pass
            );
            assert_eq!(
                h.handle(Event::ButtonUp(MouseInput::MButton), modifiers, now),
                Decision::Pass
            );
        }
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let mut h = handler();
        let now = Instant::now();
        let both = ALT.union(Modifiers::SHIFT);

        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::LButton), ALT, now),
            Decision::Pass
        );
        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::LButton), both, now),
            Decision::Fire("shift-left")
        );
        // a binding without modifiers is for the button on its own
        assert_eq!(
            h.handle(
                Event::ButtonDown(MouseInput::XButton1),
                Modifiers::NONE,
                now
            ),
            Decision::Fire("back")
        );
        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::XButton1), ALT, now),
            Decision::Pass
        );
    }

    #[test]
    fn a_wheel_notch_fires_once() {
        let mut h = handler();
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(-120), ALT, now), Decision::Fire("down"));
        assert_eq!(
            h.handle(
                Event::Wheel {
                    horizontal: true,
                    delta: 120
                },
                ALT,
                now
            ),
            Decision::Fire("right")
        );
    }

    #[test]
    fn smooth_scrolling_adds_up_to_notches() {
        let mut h = handler();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        // four quarter-notches make one
        let decisions: Vec<_> = (0..4)
            .map(|i| h.handle(wheel(30), ALT, at(i * 10)))
            .collect();
        assert_eq!(
            decisions,
            vec![
                Decision::Swallow,
                Decision::Swallow,
                Decision::Swallow,
                Decision::Fire("up")
            ]
        );
    }

    #[test]
    fn what_is_left_over_counts_towards_the_next_notch() {
        let mut h = handler();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        assert_eq!(h.handle(wheel(100), ALT, at(0)), Decision::Swallow);
        // 200 so far: fires, and 80 is kept
        assert_eq!(h.handle(wheel(100), ALT, at(10)), Decision::Fire("up"));
        // 80 + 40 = 120: fires again
        assert_eq!(h.handle(wheel(40), ALT, at(20)), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(40), ALT, at(30)), Decision::Swallow);
    }

    #[test]
    fn reversing_direction_starts_again() {
        let mut h = handler();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        assert_eq!(h.handle(wheel(100), ALT, at(0)), Decision::Swallow);
        // without the reset this would be 100 - 100 and then fire on the next up
        assert_eq!(h.handle(wheel(-100), ALT, at(10)), Decision::Swallow);
        assert_eq!(h.handle(wheel(-100), ALT, at(20)), Decision::Fire("down"));
    }

    #[test]
    fn scrolling_that_pauses_starts_again() {
        let mut h = handler();
        let start = Instant::now();

        assert_eq!(h.handle(wheel(100), ALT, start), Decision::Swallow);
        // a second later the 100 is forgotten, so this is not a notch
        assert_eq!(
            h.handle(wheel(100), ALT, start + Duration::from_secs(1)),
            Decision::Swallow
        );
        assert_eq!(
            h.handle(wheel(100), ALT, start + Duration::from_millis(1010)),
            Decision::Fire("up")
        );
    }

    #[test]
    fn scrolling_that_is_not_bound_passes_and_forgets() {
        let mut h = handler();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        // left is not bound, and without the modifier nothing is
        assert_eq!(
            h.handle(
                Event::Wheel {
                    horizontal: true,
                    delta: -120
                },
                ALT,
                at(0)
            ),
            Decision::Pass
        );
        assert_eq!(h.handle(wheel(120), Modifiers::NONE, at(0)), Decision::Pass);

        // the modifier was let go in the middle of a scroll: what was counted is gone
        assert_eq!(h.handle(wheel(100), ALT, at(10)), Decision::Swallow);
        assert_eq!(
            h.handle(wheel(100), Modifiers::NONE, at(20)),
            Decision::Pass
        );
        assert_eq!(h.handle(wheel(100), ALT, at(30)), Decision::Swallow);
    }

    #[test]
    fn the_two_wheels_count_separately() {
        let mut h = handler();
        let start = Instant::now();

        assert_eq!(h.handle(wheel(100), ALT, start), Decision::Swallow);
        // horizontal scrolling does not add to vertical
        assert_eq!(
            h.handle(
                Event::Wheel {
                    horizontal: true,
                    delta: 100
                },
                ALT,
                start
            ),
            Decision::Swallow
        );
        assert_eq!(h.handle(wheel(20), ALT, start), Decision::Fire("up"));
    }

    #[test]
    fn an_empty_handler_passes_everything() {
        let mut h: Handler<&str> = Handler::new([], Duration::ZERO);
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Pass);
        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::MButton), ALT, now),
            Decision::Pass
        );
    }

    fn word(high: i16) -> u32 {
        u32::from(high as u16) << 16
    }

    #[test]
    fn messages_are_decoded_into_events() {
        assert_eq!(
            decode(WM_LBUTTONDOWN, 0),
            Some(Event::ButtonDown(MouseInput::LButton))
        );
        assert_eq!(
            decode(WM_LBUTTONUP, 0),
            Some(Event::ButtonUp(MouseInput::LButton))
        );
        assert_eq!(
            decode(WM_RBUTTONDOWN, 0),
            Some(Event::ButtonDown(MouseInput::RButton))
        );
        assert_eq!(
            decode(WM_MBUTTONUP, 0),
            Some(Event::ButtonUp(MouseInput::MButton))
        );
    }

    #[test]
    fn the_extra_buttons_are_told_apart_by_the_high_word() {
        assert_eq!(
            decode(WM_XBUTTONDOWN, word(1)),
            Some(Event::ButtonDown(MouseInput::XButton1))
        );
        assert_eq!(
            decode(WM_XBUTTONUP, word(2)),
            Some(Event::ButtonUp(MouseInput::XButton2))
        );
        assert_eq!(decode(WM_XBUTTONDOWN, word(3)), None);
        assert_eq!(decode(WM_XBUTTONDOWN, 0), None);
    }

    #[test]
    fn wheel_deltas_are_signed() {
        assert_eq!(
            decode(WM_MOUSEWHEEL, word(120)),
            Some(Event::Wheel {
                horizontal: false,
                delta: 120
            })
        );
        assert_eq!(
            decode(WM_MOUSEWHEEL, word(-120)),
            Some(Event::Wheel {
                horizontal: false,
                delta: -120
            })
        );
        assert_eq!(
            decode(WM_MOUSEHWHEEL, word(-30)),
            Some(Event::Wheel {
                horizontal: true,
                delta: -30
            })
        );
    }

    #[test]
    fn movement_and_other_messages_are_not_events() {
        assert_eq!(decode(WM_MOUSEMOVE, 0), None);
        assert_eq!(decode(0, 0), None);
    }

    /// Starts a real hook and sends real, if synthetic, input, so it is not run by default:
    /// `cargo test -p whkd -- --ignored mouse`. Nothing else should be using the mouse meanwhile.
    #[test]
    #[ignore = "starts a real mouse hook and sends synthetic input"]
    fn synthetic_input_fires_the_bindings() {
        use crate::Trigger;
        use whkd_core::Condition;
        use win_hotkeys::HotkeyManager;
        use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
        use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_MOUSE;
        use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_WHEEL;
        use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_XDOWN;
        use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_XUP;
        use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEINPUT;
        use windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS;

        fn send(flags: MOUSE_EVENT_FLAGS, data: u32) {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: data,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        }

        let entry = |input, command: &str| -> Entries {
            Arc::new(vec![HkmData {
                trigger: Trigger::Mouse {
                    mod_keys: vec![],
                    input,
                },
                command: command.to_string(),
                condition: Condition::Always,
            }])
        };

        let (fired, seen) = mpsc::channel();
        start(
            [
                (
                    (MouseInput::XButton1, Modifiers::NONE),
                    entry(MouseInput::XButton1, "back"),
                ),
                (
                    (MouseInput::WheelUp, Modifiers::NONE),
                    entry(MouseInput::WheelUp, "up"),
                ),
            ],
            HotkeyManager::<()>::new().pause_handle(),
            Duration::ZERO,
            true,
            move |entries| fired.send(entries[0].command.clone()).unwrap(),
        )
        .unwrap();

        // the hook is installed on its own thread, so give it a moment
        std::thread::sleep(Duration::from_millis(500));

        send(MOUSEEVENTF_XDOWN, 1);
        send(MOUSEEVENTF_XUP, 1);
        assert_eq!(seen.recv_timeout(Duration::from_secs(2)).unwrap(), "back");

        send(MOUSEEVENTF_WHEEL, 120);
        assert_eq!(seen.recv_timeout(Duration::from_secs(2)).unwrap(), "up");

        // the release of the button is swallowed with its press, so that is one firing, not two
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    }

    fn with_cooldown(milliseconds: u64) -> Handler<&'static str> {
        Handler::new(
            [
                ((MouseInput::WheelUp, ALT), "up"),
                ((MouseInput::WheelDown, ALT), "down"),
                ((MouseInput::WheelRight, ALT), "right"),
            ],
            Duration::from_millis(milliseconds),
        )
    }

    #[test]
    fn momentum_after_a_firing_does_not_fire_again() {
        let mut h = with_cooldown(100);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        assert_eq!(h.handle(wheel(120), ALT, at(0)), Decision::Fire("up"));
        for ms in [10, 50, 99] {
            assert_eq!(h.handle(wheel(120), ALT, at(ms)), Decision::Swallow, "{ms}");
        }
        // over, so scrolling counts again
        assert_eq!(h.handle(wheel(120), ALT, at(100)), Decision::Fire("up"));
    }

    #[test]
    fn what_is_scrolled_during_the_cooldown_is_forgotten() {
        let mut h = with_cooldown(100);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        assert_eq!(h.handle(wheel(120), ALT, at(0)), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(100), ALT, at(50)), Decision::Swallow);
        // without forgetting, 100 + 100 would be a notch
        assert_eq!(h.handle(wheel(100), ALT, at(110)), Decision::Swallow);
        assert_eq!(h.handle(wheel(30), ALT, at(120)), Decision::Fire("up"));
    }

    #[test]
    fn the_cooldown_counts_from_the_firing_not_from_the_last_event() {
        let mut h = with_cooldown(100);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);

        // scrolling that never stops still fires now and then
        assert_eq!(h.handle(wheel(120), ALT, at(0)), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(120), ALT, at(60)), Decision::Swallow);
        assert_eq!(h.handle(wheel(120), ALT, at(120)), Decision::Fire("up"));
    }

    #[test]
    fn the_other_direction_and_the_other_wheel_are_not_held_back() {
        let mut h = with_cooldown(100);
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(-120), ALT, now), Decision::Fire("down"));
        assert_eq!(
            h.handle(
                Event::Wheel {
                    horizontal: true,
                    delta: 120
                },
                ALT,
                now
            ),
            Decision::Fire("right")
        );
        // but the same direction again is
        assert_eq!(h.handle(wheel(-120), ALT, now), Decision::Swallow);
    }

    #[test]
    fn a_cooldown_of_zero_turns_it_off() {
        let mut h = with_cooldown(0);
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Fire("up"));
        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Fire("up"));
    }

    #[test]
    fn the_cooldown_does_not_touch_buttons_or_unbound_scrolling() {
        let mut h = Handler::new(
            [
                ((MouseInput::WheelUp, ALT), "up"),
                ((MouseInput::MButton, ALT), "middle"),
            ],
            Duration::from_secs(10),
        );
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Fire("up"));
        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::MButton), ALT, now),
            Decision::Fire("middle")
        );
        // not bound, so not ours, cooldown or not
        assert_eq!(h.handle(wheel(-120), ALT, now), Decision::Pass);
    }
}
