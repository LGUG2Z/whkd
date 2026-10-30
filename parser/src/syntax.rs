//! The vocabulary of a whkdrc: every word and piece of punctuation the parser matches literally.
//!
//! The parser and the formatter use these, and the editor grammars under `editors/` are generated
//! from them (`cargo xtask editors`), so a change here reaches every editor without being copied by
//! hand. The shells and layouts come from `whkd_core` (`Shell::ALL`, `Layout::ALL`).

/// Starts a comment, which runs to the end of the line (and ends a command)
pub const COMMENT: char = '#';

/// Separates the keys of a hotkey
pub const KEY_SEPARATOR: char = '+';

/// Separates the left-hand side of a binding from its command
pub const DELIMITER: char = ':';

/// `{a,b}` and `{1-9}` in a hotkey and its command stand for several bindings
pub const GROUP_OPEN: char = '{';
pub const GROUP_CLOSE: char = '}';
pub const GROUP_SEPARATOR: char = ',';

/// `a-e` in a group is `a`, `b`, `c`, `d` and `e`
pub const RANGE_SEPARATOR: char = '-';

/// An element of a group that is nothing: `{_,shift +}`
pub const EMPTY_ELEMENT: &str = "_";

/// Before a brace in a command to make it part of the command, not a group
pub const ESCAPE: char = '\\';

pub const BLOCK_OPEN: char = '[';
pub const BLOCK_CLOSE: char = ']';

/// `app = a | b`
pub const EQUALS: char = '=';
pub const NOT_EQUALS: &str = "!=";
pub const OR: char = '|';

/// `app = a & layout = b`
pub const AND: char = '&';

pub const SHELL: &str = ".shell";
pub const PAUSE: &str = ".pause";
pub const PAUSE_HOOK: &str = ".pause_hook";
pub const WHEEL_COOLDOWN: &str = ".wheel_cooldown";
pub const GESTURE_COOLDOWN: &str = ".gesture_cooldown";
pub const IGNORE_APPS: &str = ".ignore";

/// The directives, in the order they must appear
pub const DIRECTIVES: [&str; 6] = [
    SHELL,
    PAUSE,
    PAUSE_HOOK,
    WHEEL_COOLDOWN,
    GESTURE_COOLDOWN,
    IGNORE_APPS,
];

/// `keys when condition : command`
pub const WHEN: &str = "when";

/// The block entry that applies when nothing else does
pub const DEFAULT: &str = "Default";

/// The command that does nothing, but swallows the key
pub const IGNORE: &str = "Ignore";

/// What a condition can test: `app = ...`
pub const APP: &str = "app";

/// What a condition can test: `layout = ...`
pub const LAYOUT: &str = "layout";

pub const PREDICATES: [&str; 2] = [APP, LAYOUT];

/// Words with a meaning of their own in a binding or a block
pub const KEYWORDS: [&str; 3] = [WHEN, DEFAULT, IGNORE];

#[must_use]
pub fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

#[must_use]
pub fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A character of a value in `app = ...` / `layout = ...`
#[must_use]
pub fn is_value_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
}
