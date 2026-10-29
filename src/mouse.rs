//! Mouse bindings: the pure logic.
//!
//! Which bindings apply to a mouse event, when a press or a wheel notch fires a command, and what to
//! swallow, with no Win32 in it so that it can be tested. The mouse hook feeds this the events it
//! sees and does what it says.
//!
//! A hook has to answer at once (Windows removes a hook that is slow), so what is swallowed is
//! decided from the bindings alone. Which command a binding runs, which can depend on the focused
//! application or on komorebi, is decided later, off the hook's thread.

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;
use whkd_core::MouseInput;
use win_hotkeys::VKey;

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
}

/// The bindings, and the little state that deciding about a stream of events needs
#[derive(Debug)]
pub struct Handler<T> {
    bindings: HashMap<(MouseInput, Modifiers), T>,
    /// Buttons whose press was swallowed, so that the release is too and the application never sees
    /// a release without a press
    swallowed: HashSet<MouseInput>,
    vertical: Wheel,
    horizontal: Wheel,
}

impl<T: Clone> Handler<T> {
    pub fn new(bindings: impl IntoIterator<Item = ((MouseInput, Modifiers), T)>) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            swallowed: HashSet::new(),
            vertical: Wheel::default(),
            horizontal: Wheel::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
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
            Decision::Fire(binding.clone())
        } else {
            Decision::Swallow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT: Modifiers = Modifiers::ALT;

    fn handler() -> Handler<&'static str> {
        Handler::new([
            ((MouseInput::WheelUp, ALT), "up"),
            ((MouseInput::WheelDown, ALT), "down"),
            ((MouseInput::WheelRight, ALT), "right"),
            ((MouseInput::MButton, ALT), "middle"),
            (
                (MouseInput::LButton, ALT.union(Modifiers::SHIFT)),
                "shift-left",
            ),
            ((MouseInput::XButton1, Modifiers::NONE), "back"),
        ])
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
        let mut h: Handler<&str> = Handler::new([]);
        assert!(h.is_empty());
        let now = Instant::now();

        assert_eq!(h.handle(wheel(120), ALT, now), Decision::Pass);
        assert_eq!(
            h.handle(Event::ButtonDown(MouseInput::MButton), ALT, now),
            Decision::Pass
        );
    }
}
