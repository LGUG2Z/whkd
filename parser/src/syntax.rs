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

/// The directives, in the order they must appear
pub const DIRECTIVES: [&str; 3] = [SHELL, PAUSE, PAUSE_HOOK];

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
