use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whkdrc {
    pub shell: Shell,
    pub app_bindings: Vec<(Vec<String>, Vec<HotkeyBinding>)>,
    pub bindings: Vec<HotkeyBinding>,
    pub pause_binding: Option<Vec<String>>,
    pub pause_hook: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Shell {
    Cmd,
    Powershell,
    Pwsh,
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
        match self {
            Self::Cmd => write!(f, "cmd"),
            Self::Powershell => write!(f, "powershell"),
            Self::Pwsh => write!(f, "pwsh"),
        }
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

    /// Parse a layout name, ignoring case and separators so that both `ultrawide-vertical-stack`
    /// and komorebi's own `UltrawideVerticalStack` are accepted
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let name = normalise(name);
        Self::ALL
            .into_iter()
            .find(|layout| normalise(layout.name()) == name)
    }
}

impl Display for Layout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
        for layout in Layout::ALL {
            assert_eq!(Layout::parse(layout.name()), Some(layout));
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
