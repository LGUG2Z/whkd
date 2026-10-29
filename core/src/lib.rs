use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whkdrc {
    pub shell: Shell,
    pub app_bindings: Vec<(Vec<String>, Vec<HotkeyBinding>)>,
    pub bindings: Vec<HotkeyBinding>,
    pub pause_binding: Option<Vec<String>>,
    pub pause_hook: Option<String>,
    /// How long, in milliseconds, a wheel binding ignores further scrolling after it fires, if the
    /// whkdrc says so. Otherwise it is [`DEFAULT_WHEEL_COOLDOWN_MS`]
    pub wheel_cooldown: Option<u32>,
    /// How long, in milliseconds, after a swipe with a direction another one is not told, if the
    /// whkdrc says so. Otherwise it is [`DEFAULT_GESTURE_COOLDOWN_MS`]
    pub gesture_cooldown: Option<u32>,
}

/// A trackpad keeps scrolling for a moment after the fingers lift, so without a pause between
/// them one flick could fire a binding many times. Short enough not to be felt with a mouse wheel
pub const DEFAULT_WHEEL_COOLDOWN_MS: u32 = 100;

/// Swipes are slower and rarer than notches of a wheel, and a second one right after the first is
/// usually the end of the same movement
pub const DEFAULT_GESTURE_COOLDOWN_MS: u32 = 300;

/// A cooldown longer than this is a mistake, such as microseconds written as milliseconds
pub const MAX_COOLDOWN_MS: u32 = 60_000;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Shell {
    Cmd,
    Powershell,
    Pwsh,
}

impl Shell {
    pub const ALL: [Self; 3] = [Self::Cmd, Self::Pwsh, Self::Powershell];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::Powershell => "powershell",
            Self::Pwsh => "pwsh",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shell| shell.name() == name)
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<String> for Shell {
    fn from(value: String) -> Self {
        match value.as_str() {
            "pwsh" => Self::Pwsh,
            "powershell" => Self::Powershell,
            "cmd" => Self::Cmd,
            _ => panic!("unsupported shell"),
        }
    }
}

impl Display for Shell {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub keys: Vec<String>,
    pub command: String,
    pub condition: Condition,
}

/// The built-in komorebi layouts
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Layout {
    Bsp,
    Columns,
    Rows,
    VerticalStack,
    HorizontalStack,
    UltrawideVerticalStack,
    Grid,
    RightMainVerticalStack,
    Scrolling,
}

impl Layout {
    pub const ALL: [Self; 9] = [
        Self::Bsp,
        Self::Columns,
        Self::Rows,
        Self::VerticalStack,
        Self::HorizontalStack,
        Self::UltrawideVerticalStack,
        Self::Grid,
        Self::RightMainVerticalStack,
        Self::Scrolling,
    ];

    /// The name as used by `komorebic`, e.g. `ultrawide-vertical-stack`
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bsp => "bsp",
            Self::Columns => "columns",
            Self::Rows => "rows",
            Self::VerticalStack => "vertical-stack",
            Self::HorizontalStack => "horizontal-stack",
            Self::UltrawideVerticalStack => "ultrawide-vertical-stack",
            Self::Grid => "grid",
            Self::RightMainVerticalStack => "right-main-vertical-stack",
            Self::Scrolling => "scrolling",
        }
    }

    /// Parse a layout name, ignoring case, and allowing a `-`, `_` or space (or nothing) between
    /// its words, so that both `ultrawide-vertical-stack` and komorebi's own
    /// `UltrawideVerticalStack` are accepted. Separators anywhere else are not: `s_crolling` is
    /// not a layout.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|layout| layout.is_spelt_as(name))
    }

    fn is_spelt_as(self, spelling: &str) -> bool {
        let mut rest = spelling;

        for (index, word) in self.name().split('-').enumerate() {
            if index > 0 {
                rest = rest.strip_prefix(['-', '_', ' ']).unwrap_or(rest);
            }

            match rest.get(..word.len()) {
                Some(head) if head.eq_ignore_ascii_case(word) => rest = &rest[word.len()..],
                _ => return false,
            }
        }

        rest.is_empty()
    }
}

impl Display for Layout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A mouse button or scroll direction that can trigger a binding. Two-finger scrolling on a
/// trackpad arrives as the wheel inputs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MouseInput {
    LButton,
    RButton,
    MButton,
    XButton1,
    XButton2,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

impl MouseInput {
    pub const ALL: [Self; 9] = [
        Self::LButton,
        Self::RButton,
        Self::MButton,
        Self::XButton1,
        Self::XButton2,
        Self::WheelUp,
        Self::WheelDown,
        Self::WheelLeft,
        Self::WheelRight,
    ];

    /// The name used in a whkdrc, e.g. `wheelup`
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LButton => "lbutton",
            Self::RButton => "rbutton",
            Self::MButton => "mbutton",
            Self::XButton1 => "xbutton1",
            Self::XButton2 => "xbutton2",
            Self::WheelUp => "wheelup",
            Self::WheelDown => "wheeldown",
            Self::WheelLeft => "wheelleft",
            Self::WheelRight => "wheelright",
        }
    }

    /// Parse a name from a whkdrc, ignoring case
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|input| input.name().eq_ignore_ascii_case(name))
    }

    /// The primary buttons cannot be bound on their own: swallowing every click would leave no way
    /// to click anything, including to fix the config
    #[must_use]
    pub const fn requires_modifier(self) -> bool {
        matches!(self, Self::LButton | Self::RButton)
    }

    #[must_use]
    pub const fn is_wheel(self) -> bool {
        matches!(
            self,
            Self::WheelUp | Self::WheelDown | Self::WheelLeft | Self::WheelRight
        )
    }
}

impl Display for MouseInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// The way a swipe moves
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

/// A swipe of three or four fingers on a touchpad, which can trigger a binding like a mouse input
/// can. Windows does not pass these on to programs, so whkd recognises them itself from the
/// touchpad's raw input.
///
/// With a direction (`swipe3left`) a swipe runs its command once, when the swipe is recognised.
/// Without one (`swipe3`) it is a stream: the command is started when the fingers land and is told
/// how they move until they lift, which is what scrubbing needs and a command per movement could
/// not keep up with.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum GestureInput {
    /// Three fingers, as a stream
    Swipe3,
    /// Four fingers, as a stream
    Swipe4,
    Swipe3Left,
    Swipe3Right,
    Swipe3Up,
    Swipe3Down,
    Swipe4Left,
    Swipe4Right,
    Swipe4Up,
    Swipe4Down,
}

impl GestureInput {
    pub const ALL: [Self; 10] = [
        Self::Swipe3,
        Self::Swipe4,
        Self::Swipe3Left,
        Self::Swipe3Right,
        Self::Swipe3Up,
        Self::Swipe3Down,
        Self::Swipe4Left,
        Self::Swipe4Right,
        Self::Swipe4Up,
        Self::Swipe4Down,
    ];

    /// The name used in a whkdrc, e.g. `swipe3left`
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Swipe3 => "swipe3",
            Self::Swipe4 => "swipe4",
            Self::Swipe3Left => "swipe3left",
            Self::Swipe3Right => "swipe3right",
            Self::Swipe3Up => "swipe3up",
            Self::Swipe3Down => "swipe3down",
            Self::Swipe4Left => "swipe4left",
            Self::Swipe4Right => "swipe4right",
            Self::Swipe4Up => "swipe4up",
            Self::Swipe4Down => "swipe4down",
        }
    }

    /// Parse a name from a whkdrc, ignoring case
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|input| input.name().eq_ignore_ascii_case(name))
    }

    /// How many fingers swipe: 3 or 4
    #[must_use]
    pub const fn fingers(self) -> u8 {
        match self {
            Self::Swipe3
            | Self::Swipe3Left
            | Self::Swipe3Right
            | Self::Swipe3Up
            | Self::Swipe3Down => 3,
            Self::Swipe4
            | Self::Swipe4Left
            | Self::Swipe4Right
            | Self::Swipe4Up
            | Self::Swipe4Down => 4,
        }
    }

    /// The way it moves, for a swipe that runs its command once. A stream has none: it follows the
    /// fingers wherever they go
    #[must_use]
    pub const fn direction(self) -> Option<SwipeDirection> {
        match self {
            Self::Swipe3 | Self::Swipe4 => None,
            Self::Swipe3Left | Self::Swipe4Left => Some(SwipeDirection::Left),
            Self::Swipe3Right | Self::Swipe4Right => Some(SwipeDirection::Right),
            Self::Swipe3Up | Self::Swipe4Up => Some(SwipeDirection::Up),
            Self::Swipe3Down | Self::Swipe4Down => Some(SwipeDirection::Down),
        }
    }

    /// Whether the command is started when the fingers land and told how they move, rather than
    /// run once when the swipe is recognised
    #[must_use]
    pub const fn is_stream(self) -> bool {
        self.direction().is_none()
    }

    /// The swipe for `fingers` moving in `direction`, if that many fingers have one
    #[must_use]
    pub fn swipe(fingers: u8, direction: SwipeDirection) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|input| input.fingers() == fingers && input.direction() == Some(direction))
    }

    /// The stream for `fingers`, if that many fingers have one
    #[must_use]
    pub fn stream(fingers: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|input| input.fingers() == fingers && input.is_stream())
    }
}

impl Display for GestureInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A single test against the current state, e.g. `app = Firefox` or `layout != bsp`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    App { names: Vec<String>, negated: bool },
    Layout { layouts: Vec<Layout>, negated: bool },
}

impl Predicate {
    #[must_use]
    pub fn matches(&self, state: &State) -> bool {
        match self {
            Self::App { names, negated } => names.iter().any(|name| name == state.app) != *negated,
            // when the layout is unknown, nothing can be said about it, negated or not
            Self::Layout { layouts, negated } => state
                .layout
                .is_some_and(|layout| layouts.contains(&layout) != *negated),
        }
    }
}

/// When a binding applies
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// A plain binding, outside of any `[ ... ]` block
    Always,
    /// The `Default` entry of a block: only used when no other entry matches
    Default,
    /// All of the predicates must hold
    Predicates(Vec<Predicate>),
}

impl Condition {
    pub fn app(name: impl Into<String>) -> Self {
        Self::Predicates(vec![Predicate::App {
            names: vec![name.into()],
            negated: false,
        }])
    }

    #[must_use]
    pub fn needs_layout(&self) -> bool {
        match self {
            Self::Predicates(predicates) => predicates
                .iter()
                .any(|predicate| matches!(predicate, Predicate::Layout { .. })),
            Self::Always | Self::Default => false,
        }
    }

    #[must_use]
    pub fn matches(&self, state: &State) -> bool {
        match self {
            Self::Always | Self::Default => true,
            Self::Predicates(predicates) => predicates.iter().all(|p| p.matches(state)),
        }
    }
}

/// What conditions are evaluated against
#[derive(Debug, Copy, Clone)]
pub struct State<'a> {
    /// The name of the application with the focused window
    pub app: &'a str,
    /// The layout of komorebi's focused workspace, if komorebi could be asked
    pub layout: Option<Layout>,
}

/// Pick the entry to run: the first whose condition matches. If none does, fall back to the first
/// `Default` entry (wherever in the block it appears), or failing that to the first plain binding
/// for the same keys.
pub fn select<'a, T>(
    entries: impl IntoIterator<Item = (&'a Condition, T)>,
    state: &State,
) -> Option<T> {
    let mut default = None;
    let mut always = None;

    for (condition, value) in entries {
        match condition {
            Condition::Default => {
                default.get_or_insert(value);
            }
            Condition::Always => {
                always.get_or_insert(value);
            }
            Condition::Predicates(_) if condition.matches(state) => return Some(value),
            Condition::Predicates(_) => {}
        }
    }

    default.or(always)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_is(layouts: &[Layout], negated: bool) -> Condition {
        Condition::Predicates(vec![Predicate::Layout {
            layouts: layouts.to_vec(),
            negated,
        }])
    }

    #[test]
    fn layout_names_are_normalised() {
        assert_eq!(Layout::parse("BSP"), Some(Layout::Bsp));
        assert_eq!(
            Layout::parse("UltrawideVerticalStack"),
            Some(Layout::UltrawideVerticalStack)
        );
        assert_eq!(
            Layout::parse("right-main-vertical-stack"),
            Some(Layout::RightMainVerticalStack)
        );
        assert_eq!(Layout::parse("nope"), None);

        // separators go between the words, and nowhere else
        assert_eq!(
            Layout::parse("ultrawide vertical_stack"),
            Some(Layout::UltrawideVerticalStack)
        );
        for misspelt in [
            "s_crolling",
            "bs.p",
            "scrolling_",
            "sc rolling",
            "-bsp",
            "ultrawide--vertical-stack",
            "",
        ] {
            assert_eq!(Layout::parse(misspelt), None, "{misspelt}");
        }
        for layout in Layout::ALL {
            assert_eq!(Layout::parse(layout.name()), Some(layout));
        }
    }

    #[test]
    fn mouse_inputs_parse_by_name_in_any_case() {
        for input in MouseInput::ALL {
            assert_eq!(MouseInput::parse(input.name()), Some(input));
            assert_eq!(MouseInput::parse(&input.name().to_uppercase()), Some(input));
            assert_eq!(input.to_string(), input.name());
        }

        assert_eq!(MouseInput::parse("WheelUp"), Some(MouseInput::WheelUp));
        for not_a_mouse_input in ["", "h", "mouse1", "wheel", "wheelup ", "lbutton1"] {
            assert_eq!(
                MouseInput::parse(not_a_mouse_input),
                None,
                "{not_a_mouse_input}"
            );
        }
    }

    #[test]
    fn only_the_primary_buttons_need_a_modifier() {
        let needing: Vec<_> = MouseInput::ALL
            .into_iter()
            .filter(|input| input.requires_modifier())
            .collect();
        assert_eq!(needing, vec![MouseInput::LButton, MouseInput::RButton]);
    }

    #[test]
    fn the_wheel_inputs_are_the_four_directions() {
        let wheels: Vec<_> = MouseInput::ALL
            .into_iter()
            .filter(|input| input.is_wheel())
            .map(MouseInput::name)
            .collect();
        assert_eq!(
            wheels,
            vec!["wheelup", "wheeldown", "wheelleft", "wheelright"]
        );
    }

    #[test]
    fn gestures_parse_by_name_in_any_case() {
        for input in GestureInput::ALL {
            assert_eq!(GestureInput::parse(input.name()), Some(input));
            assert_eq!(
                GestureInput::parse(&input.name().to_uppercase()),
                Some(input)
            );
            assert_eq!(input.to_string(), input.name());
        }

        assert_eq!(
            GestureInput::parse("Swipe3Left"),
            Some(GestureInput::Swipe3Left)
        );
        for not_a_gesture in [
            "",
            "swipe",
            "swipe2",
            "swipe2left",
            "swipe5up",
            "swipe3 ",
            "swipe3left ",
            "wheelup",
        ] {
            assert_eq!(GestureInput::parse(not_a_gesture), None, "{not_a_gesture}");
        }
    }

    #[test]
    fn a_gesture_is_its_fingers_and_its_direction() {
        for input in GestureInput::ALL {
            assert!(matches!(input.fingers(), 3 | 4), "{input}");
            assert!(input
                .name()
                .starts_with(&format!("swipe{}", input.fingers())));

            match input.direction() {
                Some(direction) => {
                    assert!(!input.is_stream(), "{input}");
                    assert_eq!(GestureInput::swipe(input.fingers(), direction), Some(input));
                }
                None => assert!(input.is_stream(), "{input}"),
            }
        }

        // every number of fingers has all four directions, and no other number has any
        for direction in [
            SwipeDirection::Left,
            SwipeDirection::Right,
            SwipeDirection::Up,
            SwipeDirection::Down,
        ] {
            assert!(GestureInput::swipe(3, direction).is_some());
            assert!(GestureInput::swipe(4, direction).is_some());
            assert_eq!(GestureInput::swipe(2, direction), None);
            assert_eq!(GestureInput::swipe(5, direction), None);
        }

        assert_eq!(
            GestureInput::swipe(4, SwipeDirection::Up),
            Some(GestureInput::Swipe4Up)
        );
    }

    #[test]
    fn a_stream_is_a_swipe_without_a_direction() {
        assert_eq!(GestureInput::parse("swipe3"), Some(GestureInput::Swipe3));
        assert_eq!(GestureInput::parse("SWIPE4"), Some(GestureInput::Swipe4));

        let streams: Vec<_> = GestureInput::ALL
            .into_iter()
            .filter(|input| input.is_stream())
            .collect();
        assert_eq!(streams, vec![GestureInput::Swipe3, GestureInput::Swipe4]);

        assert_eq!(GestureInput::stream(3), Some(GestureInput::Swipe3));
        assert_eq!(GestureInput::stream(4), Some(GestureInput::Swipe4));
        assert_eq!(GestureInput::stream(2), None);
        // a stream is not one of the swipes that go one way
        assert_eq!(
            GestureInput::swipe(3, SwipeDirection::Left),
            Some(GestureInput::Swipe3Left)
        );
    }

    #[test]
    fn gestures_and_mouse_inputs_have_different_names() {
        for gesture in GestureInput::ALL {
            assert_eq!(MouseInput::parse(gesture.name()), None, "{gesture}");
        }
        for mouse in MouseInput::ALL {
            assert_eq!(GestureInput::parse(mouse.name()), None, "{mouse}");
        }
    }

    #[test]
    fn first_match_wins_and_default_is_a_fallback() {
        let firefox = Condition::app("Firefox");
        let bsp = layout_is(&[Layout::Bsp], false);
        let entries = [
            (&Condition::Default, "default"),
            (&firefox, "firefox"),
            (&bsp, "bsp"),
        ];

        let state = |app, layout| State { app, layout };
        let pick = |s: State| select(entries.iter().map(|(c, v)| (*c, *v)), &s);

        assert_eq!(pick(state("Firefox", Some(Layout::Bsp))), Some("firefox"));
        assert_eq!(pick(state("Code", Some(Layout::Bsp))), Some("bsp"));
        assert_eq!(pick(state("Code", Some(Layout::Grid))), Some("default"));
        assert_eq!(pick(state("Code", None)), Some("default"));
    }

    #[test]
    fn unknown_layout_matches_nothing_even_when_negated() {
        let state = State {
            app: "x",
            layout: None,
        };
        assert!(!layout_is(&[Layout::Bsp], false).matches(&state));
        assert!(!layout_is(&[Layout::Bsp], true).matches(&state));
    }

    #[test]
    fn negation_and_conjunction() {
        let state = State {
            app: "Firefox",
            layout: Some(Layout::Rows),
        };

        assert!(layout_is(&[Layout::Bsp, Layout::Grid], true).matches(&state));
        assert!(!layout_is(&[Layout::Rows], true).matches(&state));

        let both = Condition::Predicates(vec![
            Predicate::App {
                names: vec!["Firefox".into()],
                negated: false,
            },
            Predicate::Layout {
                layouts: vec![Layout::Rows],
                negated: false,
            },
        ]);
        assert!(both.matches(&state));
        assert!(both.needs_layout());
        assert!(!Condition::app("Firefox").needs_layout());
    }

    #[test]
    fn plain_binding_is_the_last_resort() {
        let bsp = layout_is(&[Layout::Bsp], false);
        let entries = [
            (&Condition::Always, "plain"),
            (&bsp, "bsp"),
            (&Condition::Default, "default"),
        ];

        let pick = |layout| {
            let state = State { app: "x", layout };
            select(entries.iter().map(|(c, v)| (*c, *v)), &state)
        };

        assert_eq!(pick(Some(Layout::Bsp)), Some("bsp"));
        assert_eq!(pick(Some(Layout::Grid)), Some("default"));

        let without_default = [(&Condition::Always, "plain"), (&bsp, "bsp")];
        let state = State {
            app: "x",
            layout: None,
        };
        assert_eq!(
            select(without_default.iter().map(|(c, v)| (*c, *v)), &state),
            Some("plain")
        );
    }
}
