use ariadne::Config;
use ariadne::IndexType;
use ariadne::Label;
use ariadne::Report;
use ariadne::ReportKind;
use ariadne::Source;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;
use whkd_core::Check as CheckDefinition;
use whkd_core::Condition;
use whkd_core::GestureInput;
use whkd_core::HotkeyBinding;
use whkd_core::Layout;
use whkd_core::MouseInput;
use whkd_core::Predicate;
use whkd_core::Shell;
use whkd_core::Whkdrc;
use whkd_core::MAX_COOLDOWN_MS;

mod braces;
pub mod syntax;

use syntax::is_ident_continue;
use syntax::is_ident_start;
use syntax::is_value_char;
use syntax::AND;
use syntax::APP;
use syntax::BLOCK_CLOSE;
use syntax::BLOCK_OPEN;
use syntax::COMMENT;
use syntax::DEFAULT;
use syntax::DEFINE_CHECK;
use syntax::DELIMITER;
use syntax::DIRECTIVES;
use syntax::EQUALS;
use syntax::GESTURE_COOLDOWN;
use syntax::GROUP_OPEN;
use syntax::IGNORE;
use syntax::IGNORE_APPS;
use syntax::KEY_SEPARATOR;
use syntax::LAYOUT;
use syntax::NOT;
use syntax::NOT_EQUALS;
use syntax::OR;
use syntax::PAUSE;
use syntax::PAUSE_HOOK;
use syntax::PREDICATES;
use syntax::SHELL;
use syntax::WHEEL_COOLDOWN;
use syntax::WHEN;

pub mod format;

#[derive(Debug, Error)]
pub enum WhkdError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{}", .error.render(.path, .src))]
    Parse {
        path: PathBuf,
        src: String,
        error: Box<ParseError>,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Advice,
}

/// A problem in a whkdrc (a syntax error, or a warning about something suspicious), with
/// byte-offset spans into the source
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub severity: Severity,
    pub message: String,
    pub span: Range<usize>,
    pub label: String,
    pub notes: Vec<(Range<usize>, String)>,
    pub help: Option<String>,
}

impl ParseError {
    fn new(message: impl Into<String>, span: Range<usize>, label: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            span,
            label: label.into(),
            notes: vec![],
            help: None,
        }
    }

    fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    fn with_note(mut self, span: Range<usize>, label: impl Into<String>) -> Self {
        self.notes.push((span, label.into()));
        self
    }

    fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Render this error for `path`, coloured if stderr is a terminal
    #[must_use]
    pub fn render(&self, path: &Path, src: &str) -> String {
        self.report(
            &path.display().to_string(),
            src,
            std::io::stderr().is_terminal(),
        )
    }

    /// Render this error as an ariadne report
    #[must_use]
    pub fn report(&self, name: &str, src: &str, color: bool) -> String {
        let id = name.to_string();
        let span = (id.clone(), self.span.clone());

        let mut builder = Report::build(
            match self.severity {
                Severity::Error => ReportKind::Error,
                Severity::Warning => ReportKind::Warning,
                Severity::Advice => ReportKind::Advice,
            },
            span.clone(),
        )
        .with_config(
            Config::default()
                .with_color(color)
                .with_index_type(IndexType::Byte),
        )
        .with_message(&self.message)
        .with_label(Label::new(span).with_message(&self.label));

        for (note_span, note) in &self.notes {
            builder =
                builder.with_label(Label::new((id.clone(), note_span.clone())).with_message(note));
        }

        if let Some(help) = &self.help {
            builder = builder.with_help(help);
        }

        let mut buffer = Vec::new();
        if builder
            .finish()
            .write((id, Source::from(src)), &mut buffer)
            .is_err()
        {
            let (line, column) = line_column(src, self.span.start);
            return format!("{name}:{line}:{column}: {}", self.message);
        }

        String::from_utf8_lossy(&buffer).into_owned()
    }
}

fn line_column(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

pub fn load(path: &PathBuf) -> Result<Whkdrc, WhkdError> {
    let contents = std::fs::read_to_string(path)?;

    parse(&contents).map_err(|error| WhkdError::Parse {
        path: path.clone(),
        src: contents,
        error: Box::new(error),
    })
}

/// Like [`load`], but also rejects key names that `keys` does not recognise
pub fn load_with(path: &PathBuf, keys: &KeyValidator) -> Result<Whkdrc, WhkdError> {
    let contents = std::fs::read_to_string(path)?;

    parse_with(&contents, keys).map_err(|error| WhkdError::Parse {
        path: path.clone(),
        src: contents,
        error: Box::new(error),
    })
}

/// A successfully parsed whkdrc, along with anything suspicious found in it
#[derive(Debug)]
pub struct Parsed {
    pub whkdrc: Whkdrc,
    pub warnings: Vec<ParseError>,
}

/// A whkdrc loaded from disk, with the means to render its warnings
#[derive(Debug)]
pub struct Loaded {
    pub whkdrc: Whkdrc,
    pub warnings: Vec<ParseError>,
    path: PathBuf,
    src: String,
}

impl Loaded {
    /// Each warning rendered for the file it came from
    #[must_use]
    pub fn render_warnings(&self) -> Vec<String> {
        self.warnings
            .iter()
            .map(|warning| warning.render(&self.path, &self.src))
            .collect()
    }
}

/// Like [`load_with`], but keeps the warnings too
pub fn load_checked(path: &PathBuf, keys: &KeyValidator) -> Result<Loaded, WhkdError> {
    let contents = std::fs::read_to_string(path)?;

    match parse_full(&contents, Some(keys)) {
        Ok(Parsed { whkdrc, warnings }) => Ok(Loaded {
            whkdrc,
            warnings,
            path: path.clone(),
            src: contents,
        }),
        Err(error) => Err(WhkdError::Parse {
            path: path.clone(),
            src: contents,
            error: Box::new(error),
        }),
    }
}

/// Parse the contents of a whkdrc, without checking that key names are real keys
pub fn parse(src: &str) -> Result<Whkdrc, ParseError> {
    parse_full(src, None).map(|parsed| parsed.whkdrc)
}

/// Parse the contents of a whkdrc, reporting unknown key names with a suggestion where possible
pub fn parse_with(src: &str, keys: &KeyValidator) -> Result<Whkdrc, ParseError> {
    parse_full(src, Some(keys)).map(|parsed| parsed.whkdrc)
}

/// Parse the contents of a whkdrc, also reporting warnings about things that parse but are
/// probably not what was meant
pub fn parse_full(src: &str, keys: Option<&KeyValidator>) -> Result<Parsed, ParseError> {
    Cursor {
        src,
        pos: 0,
        keys,
        entries: vec![],
        warnings: vec![],
        defined: vec![],
        used: HashSet::new(),
    }
    .parse_file()
}

type Check = Box<dyn Fn(&str) -> bool>;

/// Knows which key names are valid, so the parser can point at bad ones in the whkdrc
pub struct KeyValidator {
    is_valid: Check,
    known: Vec<String>,
    synonyms: Vec<(String, String)>,
    is_modifier: Option<Check>,
}

impl KeyValidator {
    /// `is_valid` is the source of truth; `known` is only used to suggest corrections
    pub fn new(is_valid: impl Fn(&str) -> bool + 'static, known: Vec<String>) -> Self {
        Self {
            is_valid: Box::new(is_valid),
            known,
            synonyms: vec![],
            is_modifier: None,
        }
    }

    /// Say which keys are modifiers, so that the keys held with a mouse input can be checked: only
    /// modifiers can be, because only they can be seen from a mouse hook
    #[must_use]
    pub fn with_modifiers(mut self, is_modifier: impl Fn(&str) -> bool + 'static) -> Self {
        self.is_modifier = Some(Box::new(is_modifier));
        self
    }

    /// Names people commonly try that are not valid, mapped to the valid name to suggest instead
    #[must_use]
    pub fn with_synonyms(mut self, synonyms: &[(&str, &str)]) -> Self {
        self.synonyms = synonyms
            .iter()
            .filter(|(_, valid)| (self.is_valid)(valid))
            .map(|(synonym, valid)| (synonym.to_lowercase(), (*valid).to_string()))
            .collect();
        self
    }

    fn unknown_key(&self, key: &str, span: Range<usize>) -> ParseError {
        let error = ParseError::new(format!("unknown key `{key}`"), span, "not a valid key name");

        match self.suggest(key) {
            Some(suggestion) => error.with_help(format!("did you mean `{suggestion}`?")),
            None => error,
        }
    }

    /// The closest known key name, if any is close enough to be a plausible typo
    fn suggest(&self, key: &str) -> Option<&str> {
        let key = key.to_lowercase();

        if let Some((_, valid)) = self.synonyms.iter().find(|(synonym, _)| *synonym == key) {
            return Some(valid);
        }

        closest(&key, self.known.iter().map(String::as_str))
    }
}

/// The candidate closest to `key`, if any is close enough to be a plausible typo
fn closest<'a>(key: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let max = (key.chars().count() / 3).max(1);

    candidates
        .into_iter()
        .map(|name| (edit_distance(key, name), name))
        .filter(|(distance, _)| *distance <= max)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name)
}

/// Edit distance between `a` and `b`, counting a swap of adjacent characters as one edit
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];

    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }

    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);

            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }

    d[a.len()][b.len()]
}

struct Cursor<'a> {
    src: &'a str,
    pos: usize,
    keys: Option<&'a KeyValidator>,
    /// Every binding seen, in file order, for the checks that run once parsing is done
    entries: Vec<Entry>,
    warnings: Vec<ParseError>,
    /// The conditions that `.when` defines, and where
    defined: Vec<(String, Range<usize>)>,
    /// Which of them have been used
    used: HashSet<String>,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> &'a str {
        let rest = self.rest();
        let len = rest.find(|c| !predicate(c)).unwrap_or(rest.len());
        self.pos += len;
        &rest[..len]
    }

    fn skip_inline(&mut self) {
        self.take_while(|c| c == ' ' || c == '\t');
    }

    /// Skip whitespace, a leading BOM and `#` comments
    fn skip_trivia(&mut self) {
        loop {
            self.take_while(|c| c.is_whitespace() || c == '\u{feff}');
            if self.peek() == Some(COMMENT) {
                self.take_while(|c| c != '\n');
            } else {
                break;
            }
        }
    }

    /// The span of the character under the cursor, or of the last character at the end of input
    fn here(&self) -> Range<usize> {
        match self.peek() {
            Some(c) => self.pos..self.pos + c.len_utf8(),
            None => {
                let start = self.src[..self.pos]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index);
                start..self.pos
            }
        }
    }

    /// The offset just past the last thing parsed, ignoring whitespace skipped after it
    fn trimmed_end(&self) -> usize {
        self.src[..self.pos].trim_end().len()
    }

    fn found(&self) -> String {
        match self.peek() {
            None => String::from("end of file"),
            Some('\n' | '\r') => String::from("end of line"),
            Some(c) => format!("`{c}`"),
        }
    }

    fn at_keyword(&self, name: &str) -> bool {
        self.rest()
            .strip_prefix(name)
            .is_some_and(|after| !after.starts_with(is_ident_continue))
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(ParseError::new(
                format!("expected `{c}`, found {}", self.found()),
                self.here(),
                format!("expected `{c}` here"),
            ))
        }
    }

    fn parse_file(&mut self) -> Result<Parsed, ParseError> {
        self.skip_trivia();
        let shell = self.parse_shell()?;
        self.skip_trivia();

        let pause_binding = if self.at_keyword(PAUSE) {
            self.pos += PAUSE.len();
            self.skip_inline();
            let keys = self.parse_hotkeys(false)?;
            self.skip_trivia();
            Some(keys)
        } else {
            None
        };

        let pause_hook = if self.at_keyword(PAUSE_HOOK) {
            self.pos += PAUSE_HOOK.len();
            let command = self.parse_command();
            self.skip_trivia();
            Some(command)
        } else {
            None
        };

        let wheel_cooldown = if self.at_keyword(WHEEL_COOLDOWN) {
            self.pos += WHEEL_COOLDOWN.len();
            self.skip_inline();
            let milliseconds = self.parse_milliseconds()?;
            self.skip_trivia();
            Some(milliseconds)
        } else {
            None
        };

        let gesture_cooldown = if self.at_keyword(GESTURE_COOLDOWN) {
            self.pos += GESTURE_COOLDOWN.len();
            self.skip_inline();
            let milliseconds = self.parse_milliseconds()?;
            self.skip_trivia();
            Some(milliseconds)
        } else {
            None
        };

        let ignore = if self.at_keyword(IGNORE_APPS) {
            self.pos += IGNORE_APPS.len();
            let values = self.parse_values().map_err(|error| {
                error.with_help("list the process names, as in `.ignore Firefox | Google Chrome`")
            })?;
            self.skip_trivia();
            values.into_iter().map(|(name, _)| name).collect()
        } else {
            vec![]
        };

        let mut checks = vec![];
        while self.at_keyword(DEFINE_CHECK) {
            checks.push(self.parse_check_definition()?);
            self.skip_trivia();
        }

        let mut app_bindings = vec![];
        let mut bindings = vec![];

        while self.peek().is_some() {
            let hotkey = self.parse_hotkey(true)?;
            let keys_span = hotkey.span.clone();
            self.skip_inline();

            if self.peek() == Some(BLOCK_OPEN) {
                app_bindings.extend(self.parse_app_binding(&hotkey)?);
            } else if self.peek() == Some(DELIMITER) {
                self.pos += 1;
                let commands = self.parse_commands(&hotkey)?;

                for (keys, command) in hotkey.expansions.iter().zip(commands) {
                    self.entries
                        .push(Entry::new(keys, keys_span.clone(), EntryKind::Plain));
                    bindings.push(HotkeyBinding {
                        keys: keys.clone(),
                        command,
                        condition: Condition::Always,
                    });
                }
            } else if self.at_keyword(WHEN) {
                self.pos += WHEN.len();
                self.skip_inline();

                let condition_start = self.pos;
                let condition = self.parse_condition()?;
                if condition == Condition::Default {
                    return Err(ParseError::new(
                        "`Default` cannot be used after `when`",
                        condition_start..self.trimmed_end(),
                        "a binding without `when` is already the default",
                    )
                    .with_help("write the default as a plain `keys : command` binding"));
                }

                self.skip_inline();
                self.expect(DELIMITER)?;
                let commands = self.parse_commands(&hotkey)?;

                for (keys, command) in hotkey.expansions.iter().zip(commands) {
                    self.entries.push(Entry::new(
                        keys,
                        keys_span.clone(),
                        EntryKind::Conditional(condition.clone()),
                    ));
                    app_bindings.push((
                        keys.clone(),
                        vec![HotkeyBinding {
                            keys: keys.clone(),
                            command,
                            condition: condition.clone(),
                        }],
                    ));
                }
            } else {
                return Err(ParseError::new(
                    format!(
                        "expected `:`, `[` or `when` after hotkey, found {}",
                        self.found()
                    ),
                    self.here(),
                    "expected `:`, `[` or `when` here",
                ));
            }

            self.skip_trivia();
        }

        if app_bindings.is_empty() && bindings.is_empty() {
            return Err(ParseError::new(
                "expected at least one hotkey binding",
                self.here(),
                "no bindings found",
            )
            .with_help("add a binding such as `alt + h : echo hello`"));
        }

        let mut warnings = std::mem::take(&mut self.warnings);
        warnings.extend(lint(&self.entries));
        warnings.extend(lint_ignored(&self.entries, &ignore));

        for (name, span) in &self.defined {
            if !self.used.contains(name) {
                warnings.push(
                    ParseError::new(
                        format!("`{name}` is defined but never used"),
                        span.clone(),
                        "no `when` guard or block entry uses it",
                    )
                    .with_severity(Severity::Warning)
                    .with_help(format!("use it as `when {name}`, or remove it")),
                );
            }
        }
        warnings.sort_by_key(|warning| warning.span.start);

        Ok(Parsed {
            whkdrc: Whkdrc {
                shell,
                app_bindings,
                bindings,
                pause_binding,
                pause_hook,
                wheel_cooldown,
                gesture_cooldown,
                ignore,
                checks,
            },
            warnings,
        })
    }

    /// `.when name : command`: a condition that holds when the command, run in `cmd`, exits with 0
    fn parse_check_definition(&mut self) -> Result<CheckDefinition, ParseError> {
        self.pos += DEFINE_CHECK.len();
        self.skip_inline();

        let start = self.pos;
        let name = self.take_while(is_ident_continue);
        let span = if name.is_empty() {
            self.here()
        } else {
            start..self.pos
        };

        if !name.starts_with(is_ident_start) {
            return Err(ParseError::new(
                if name.is_empty() {
                    format!(
                        "expected a name after `{DEFINE_CHECK}`, found {}",
                        self.found()
                    )
                } else {
                    format!("`{name}` cannot be the name of a condition")
                },
                span,
                "expected a name made of letters, digits and `_`, that starts with a letter",
            )
            .with_help(format!("write `{DEFINE_CHECK} docked : command`")));
        }

        // names that already mean something in a condition
        if [DEFAULT, IGNORE, APP, LAYOUT, WHEN].contains(&name) {
            return Err(ParseError::new(
                format!("`{name}` cannot be the name of a condition"),
                span,
                "it already means something in a condition",
            )
            .with_help("choose another name"));
        }

        if let Some((_, first)) = self.defined.iter().find(|(defined, _)| defined == name) {
            return Err(ParseError::new(
                format!("`{name}` is already defined"),
                span,
                "defined again here",
            )
            .with_note(first.clone(), "defined here"));
        }

        self.skip_inline();
        self.expect(DELIMITER)
            .map_err(|error| error.with_help(format!("write `{DEFINE_CHECK} {name} : command`")))?;

        self.skip_inline();
        if matches!(self.peek(), None | Some('\n' | '\r' | COMMENT)) {
            return Err(ParseError::new(
                format!("`{name}` has no command"),
                self.here(),
                "expected the command that decides it",
            )
            .with_help(
                "the condition holds when the command, which is run in `cmd`, exits with 0",
            ));
        }

        let command = self.parse_command();
        self.defined.push((name.to_string(), span));

        Ok(CheckDefinition {
            name: name.to_string(),
            command,
        })
    }

    /// A whole number of milliseconds
    fn parse_milliseconds(&mut self) -> Result<u32, ParseError> {
        let start = self.pos;
        let word = self.take_while(|c| !c.is_whitespace() && c != COMMENT);
        let span = if word.is_empty() {
            self.here()
        } else {
            start..self.pos
        };

        let Some(milliseconds) = Some(word)
            .filter(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()))
            .and_then(|word| word.parse::<u32>().ok())
        else {
            return Err(ParseError::new(
                if word.is_empty() {
                    format!("expected a number of milliseconds, found {}", self.found())
                } else {
                    format!("expected a number of milliseconds, found `{word}`")
                },
                span,
                "expected a whole number such as `150`",
            )
            .with_help("the cooldown is in milliseconds, and `0` turns it off"));
        };

        if milliseconds > MAX_COOLDOWN_MS {
            return Err(ParseError::new(
                format!("a cooldown of {milliseconds} milliseconds is too long"),
                span,
                format!("at most {MAX_COOLDOWN_MS}"),
            )
            .with_help("the cooldown is in milliseconds"));
        }

        Ok(milliseconds)
    }

    fn parse_shell(&mut self) -> Result<Shell, ParseError> {
        if !self.at_keyword(SHELL) {
            return Err(ParseError::new(
                "missing `.shell` directive",
                self.here(),
                "expected `.shell` here",
            )
            .with_help("start your whkdrc with e.g. `.shell pwsh`"));
        }

        self.pos += SHELL.len();
        self.skip_inline();

        let start = self.pos;
        let word = self.take_while(is_ident_continue);
        match Shell::parse(word) {
            Some(shell) => Ok(shell),
            None => {
                let other = word;
                let span = if other.is_empty() {
                    self.here()
                } else {
                    start..self.pos
                };

                Err(ParseError::new(
                    if other.is_empty() {
                        String::from("expected a shell after `.shell`")
                    } else {
                        format!("unknown shell `{other}`")
                    },
                    span,
                    format!(
                        "expected one of: {}",
                        Shell::ALL.map(Shell::name).join(" | ")
                    ),
                ))
            }
        }
    }

    /// The hotkey of a binding: keys, or if it has `{...}` groups, all the keys they stand for
    fn parse_hotkey(&mut self, allow_mouse: bool) -> Result<Hotkey, ParseError> {
        let start = self.pos;

        if !self.hotkey_has_groups() {
            let keys = self.parse_hotkeys(allow_mouse)?;
            return Ok(Hotkey {
                expansions: vec![keys],
                groups: vec![],
                span: start..self.trimmed_end(),
            });
        }

        let mut pieces = vec![];

        loop {
            let text_start = self.pos;
            let run = self
                .take_while(|c| !matches!(c, GROUP_OPEN | DELIMITER | BLOCK_OPEN | COMMENT | '\n'));

            // `when` ends the hotkey, unless it is a key, which is what it is after a `+`
            let (text, guarded) = match guard_start(run) {
                Some(at) => {
                    self.pos = text_start + at;
                    (&run[..at], true)
                }
                None => (run, false),
            };

            if !text.is_empty() {
                pieces.push(braces::Piece::Text {
                    text: text.to_string(),
                    start: text_start,
                });
            }

            if guarded || self.peek() != Some(GROUP_OPEN) {
                break;
            }

            let open = self.pos;
            let (length, elements) =
                braces::scan_group(self.rest()).map_err(|error| group_error(&error, open))?;

            pieces.push(braces::Piece::Group(braces::Group {
                span: open..open + length,
                elements: elements
                    .into_iter()
                    .map(|element| braces::Element {
                        text: element.text,
                        span: open + element.span.start..open + element.span.end,
                    })
                    .collect(),
            }));
            self.pos += length;
        }

        let span = start..self.trimmed_end();
        let groups: Vec<braces::Group> = pieces
            .iter()
            .filter_map(|piece| match piece {
                braces::Piece::Group(group) => Some(group.clone()),
                braces::Piece::Text { .. } => None,
            })
            .collect();

        let lengths: Vec<usize> = groups.iter().map(|group| group.elements.len()).collect();
        let count = braces::expansions(&lengths).unwrap_or(usize::MAX);
        if count > braces::MAX_EXPANSIONS {
            return Err(ParseError::new(
                format!(
                    "this hotkey stands for more than {} bindings",
                    braces::MAX_EXPANSIONS
                ),
                span,
                if count == usize::MAX {
                    String::from("far too many")
                } else {
                    format!("{count} bindings")
                },
            )
            .with_help("use fewer groups, or fewer elements in them"));
        }

        let mut expansions = vec![];
        for choice in braces::choices(&lengths) {
            let expanded = braces::expand_hotkey(&pieces, &choice);
            expansions.push(self.parse_expanded_hotkey(&expanded, &span, allow_mouse)?);
        }

        Ok(Hotkey {
            expansions,
            groups,
            span,
        })
    }

    /// Whether the hotkey that starts here has a group in it: a `{` before the end of what is
    /// before the command
    fn hotkey_has_groups(&self) -> bool {
        self.rest()
            .chars()
            .take_while(|&c| !matches!(c, DELIMITER | BLOCK_OPEN | COMMENT | '\n'))
            .any(|c| c == GROUP_OPEN)
    }

    /// The keys of a hotkey that groups have been expanded in. They are read as keys written out in
    /// full are, and a mistake in them is where the text it came from is
    fn parse_expanded_hotkey(
        &self,
        expanded: &braces::Expanded,
        whole: &Range<usize>,
        allow_mouse: bool,
    ) -> Result<Vec<String>, ParseError> {
        let mut cursor = Cursor {
            src: &expanded.text,
            pos: 0,
            keys: self.keys,
            entries: vec![],
            warnings: vec![],
            defined: vec![],
            used: HashSet::new(),
        };

        let result = cursor.parse_hotkeys(allow_mouse).and_then(|keys| {
            cursor.skip_inline();
            match cursor.peek() {
                None => Ok(keys),
                Some(_) => Err(ParseError::new(
                    format!("expected `+` between keys, found {}", cursor.found()),
                    cursor.here(),
                    "expected `+` here",
                )),
            }
        });

        result.map_err(|mut error| {
            error.span = expanded.locate(&error.span, whole);
            for (span, _) in &mut error.notes {
                *span = expanded.locate(span, whole);
            }

            error.with_note(
                whole.clone(),
                format!("when this stands for `{}`", expanded.text.trim()),
            )
        })
    }

    /// The command of a binding with `hotkey`, one for each of the keys the hotkey stands for
    fn parse_commands(&mut self, hotkey: &Hotkey) -> Result<Vec<String>, ParseError> {
        self.skip_inline();
        let start = self.pos;
        let command = self.parse_command();
        let span = start..self.trimmed_end().max(start);

        if hotkey.groups.is_empty() {
            return Ok(vec![command]);
        }

        let lengths: Vec<usize> = hotkey
            .groups
            .iter()
            .map(|group| group.elements.len())
            .collect();

        braces::expand_command(&command, &hotkey.groups, &braces::choices(&lengths)).map_err(
            |error| {
                // a command that is a line of the file is where it is. One that was joined from
                // several lines is only known as a whole
                let at = if span.len() == command.len() {
                    span.start + error.span.start..span.start + error.span.end
                } else {
                    span
                };

                let mut report = ParseError::new(error.message, at, error.label);
                report.help = error.help;
                if let Some((index, label)) = error.hotkey_group {
                    report = report.with_note(hotkey.groups[index].span.clone(), label);
                }
                report
            },
        )
    }

    /// The keys of a hotkey. Mouse inputs are allowed where `allow_mouse` is, which is wherever the
    /// hotkey is not handed to the keyboard hook
    fn parse_hotkeys(&mut self, allow_mouse: bool) -> Result<Vec<String>, ParseError> {
        let mut keys = vec![];
        let mut spans = vec![];

        loop {
            let start = self.pos;
            keys.push(self.parse_key()?);
            spans.push(start..self.pos);

            self.skip_inline();
            if !self.eat(KEY_SEPARATOR) {
                break;
            }

            self.skip_inline();
        }

        check_mouse_inputs(&keys, &spans, allow_mouse)?;
        self.check_mouse_modifiers(&keys, &spans)?;
        Ok(keys)
    }

    /// The keys held with a mouse input have to be modifiers, if it is known which keys are
    fn check_mouse_modifiers(
        &self,
        keys: &[String],
        spans: &[Range<usize>],
    ) -> Result<(), ParseError> {
        let (Some(validator), Some(last)) = (self.keys, keys.last()) else {
            return Ok(());
        };
        let Some(is_modifier) = &validator.is_modifier else {
            return Ok(());
        };
        let Some(input) = PointerInput::parse(last) else {
            return Ok(());
        };

        let held = &keys[..keys.len() - 1];
        match held.iter().zip(spans).find(|(key, _)| !is_modifier(key)) {
            Some((key, span)) => Err(ParseError::new(
                format!("`{key}` cannot be held down with `{}`", input.name()),
                span.clone(),
                "not a modifier",
            )
            .with_help(format!(
                "only alt, ctrl, shift and win can be used with a {}",
                input.kind()
            ))),
            None => Ok(()),
        }
    }

    fn parse_key(&mut self) -> Result<String, ParseError> {
        let start = self.pos;

        let key = match self.peek() {
            Some(c) if is_ident_start(c) => self.take_while(is_ident_continue),
            Some(c) if c.is_ascii_digit() => self.take_while(|c| c.is_ascii_digit()),
            found => {
                let error = ParseError::new(
                    format!("expected a key name, found {}", self.found()),
                    self.here(),
                    "expected a key name here",
                );

                return Err(if found == Some('.') {
                    error.with_help(format!(
                        "directives ({}) must come first, in that order",
                        DIRECTIVES
                            .map(|directive| format!("`{directive}`"))
                            .join(", ")
                    ))
                } else {
                    error
                });
            }
        };

        if let Some(keys) = self.keys {
            if !(keys.is_valid)(key) {
                return Err(keys.unknown_key(key, start..self.pos));
            }
        }

        Ok(key.to_string())
    }

    /// The rest of the line, up to a comment, trimmed. An empty command is accepted, as it always
    /// has been, but flagged
    ///
    /// A `\` that is the last thing on a line, after whitespace, continues the command on the next
    /// line, and the lines are joined with a single space. A `\` that ends a word, as in
    /// `explorer C:\Users\`, is part of the command, as it always has been
    fn parse_command(&mut self) -> String {
        self.skip_inline();

        let mut command = String::new();

        loop {
            let start = self.pos;
            let line = self.take_while(|c| c != '\n' && c != COMMENT).trim_end();
            let continued = continuation(line).filter(|_| matches!(self.peek(), Some('\n') | None));

            if !command.is_empty() && !continued.unwrap_or(line).is_empty() {
                command.push(' ');
            }
            command.push_str(continued.unwrap_or(line));

            if continued.is_none() {
                break;
            }

            // the `\` is the last thing in the trimmed line
            let backslash = start + line.len() - 1;

            if self.peek().is_some() {
                self.pos += 1;
                self.skip_inline();
            }

            if matches!(self.peek(), None | Some('\n' | '\r' | COMMENT)) {
                self.warnings.push(
                    ParseError::new(
                        "nothing to continue onto",
                        backslash..backslash + 1,
                        "this `\\` continues the command, but the next line is empty or a comment",
                    )
                    .with_severity(Severity::Warning)
                    .with_help("remove the `\\`, or put the rest of the command on the next line"),
                );
                break;
            }
        }

        if command.is_empty() {
            let span = self.here();
            self.warnings.push(
                ParseError::new("empty command", span, "there is nothing to run here")
                    .with_severity(Severity::Warning)
                    .with_help("add a command, or remove the binding"),
            );
        }

        command
    }

    /// The entries of a block, for each of the keys that its hotkey stands for
    fn parse_app_binding(&mut self, hotkey: &Hotkey) -> Result<BlockBindings, ParseError> {
        let open = self.pos;
        self.pos += 1;

        let mut mapped: Vec<Vec<HotkeyBinding>> = vec![vec![]; hotkey.expansions.len()];

        loop {
            self.skip_trivia();

            if self.eat(BLOCK_CLOSE) {
                break;
            }

            if self.peek().is_none() {
                return Err(ParseError::new(
                    "unterminated `[`",
                    self.here(),
                    "expected `]` before the end of the file",
                )
                .with_note(open..open + 1, "`[` opened here"));
            }

            let condition_start = self.pos;
            let condition = self.parse_condition()?;
            let condition_span = condition_start..self.trimmed_end();
            self.skip_inline();
            self.expect(DELIMITER).map_err(|error| {
                error.with_note(open..open + 1, "inside this `[` block; is a `]` missing?")
            })?;
            let commands = self.parse_commands(hotkey)?;

            let kind = match &condition {
                Condition::Default => EntryKind::Default,
                condition => EntryKind::Conditional(condition.clone()),
            };

            for ((keys, command), entries) in
                hotkey.expansions.iter().zip(commands).zip(&mut mapped)
            {
                self.entries
                    .push(Entry::new(keys, condition_span.clone(), kind.clone()));

                entries.push(HotkeyBinding {
                    keys: keys.clone(),
                    command,
                    condition: condition.clone(),
                });
            }
        }

        if mapped[0].is_empty() {
            return Err(ParseError::new(
                "empty app binding",
                open..self.pos,
                "expected at least one `condition : command` entry",
            ));
        }

        Ok(hotkey.expansions.iter().cloned().zip(mapped).collect())
    }

    /// `Default`, or one or more terms joined by `&`
    fn parse_condition(&mut self) -> Result<Condition, ParseError> {
        let start = self.pos;
        let mut terms = vec![self.parse_term()?];

        loop {
            self.skip_inline();
            if !self.eat(AND) {
                break;
            }

            self.skip_inline();
            terms.push(self.parse_term()?);
        }

        let mut predicates = vec![];
        let mut default = false;

        for term in terms {
            match term {
                Term::Default => default = true,
                Term::Predicate(predicate) => predicates.push(predicate),
            }
        }

        match (default, predicates.is_empty()) {
            (true, true) => Ok(Condition::Default),
            (false, _) => Ok(Condition::Predicates(predicates)),
            (true, false) => Err(ParseError::new(
                "`Default` cannot be combined with other conditions",
                start..self.pos,
                "`Default` applies when nothing else matches",
            )),
        }
    }

    /// A bare process name, `Default`, or `app = ...` / `layout = ...` (also `!=`)
    fn parse_term(&mut self) -> Result<Term, ParseError> {
        if self.peek() == Some(NOT) && !self.rest().starts_with(NOT_EQUALS) {
            return self.parse_negated_check();
        }

        if !self.peek().is_some_and(is_ident_start) {
            return Err(ParseError::new(
                format!("expected a condition, found {}", self.found()),
                self.here(),
                "expected a process name, `Default`, `app = ...` or `layout = ...` here",
            ));
        }

        let start = self.pos;
        let mut words = vec![];
        let mut end;

        loop {
            words.push(self.take_while(is_ident_continue));
            end = self.pos;
            self.skip_inline();

            if !self.peek().is_some_and(is_ident_start) {
                break;
            }
        }

        let name = words.join(" ");

        let negated = if self.rest().starts_with(NOT_EQUALS) {
            self.pos += NOT_EQUALS.len();
            true
        } else if self.eat(EQUALS) {
            false
        } else {
            return Ok(if name == DEFAULT {
                Term::Default
            } else if self.defined.iter().any(|(defined, _)| *defined == name) {
                // what was defined is what the name means, rather than a process called that, which
                // `app = ...` is for
                self.used.insert(name.clone());
                Term::Predicate(Predicate::Check {
                    name,
                    negated: false,
                })
            } else {
                Term::Predicate(Predicate::App {
                    names: vec![name],
                    negated: false,
                })
            });
        };

        let values = self.parse_values()?;

        match name.as_str() {
            APP => Ok(Term::Predicate(Predicate::App {
                names: values.into_iter().map(|(value, _)| value).collect(),
                negated,
            })),
            LAYOUT => {
                let layouts = values
                    .into_iter()
                    .map(|(value, span)| {
                        Layout::parse(&value).ok_or_else(|| unknown_layout(&value, span))
                    })
                    .collect::<Result<_, _>>()?;

                Ok(Term::Predicate(Predicate::Layout { layouts, negated }))
            }
            // what a condition that was defined writes, which is for the whkdrc to say
            _ if self.defined.iter().any(|(defined, _)| *defined == name) => {
                self.used.insert(name.clone());

                Ok(Term::Predicate(Predicate::CheckIs {
                    name,
                    values: values.into_iter().map(|(value, _)| value).collect(),
                    negated,
                }))
            }
            _ => {
                let defined: Vec<&str> = self
                    .defined
                    .iter()
                    .map(|(defined, _)| defined.as_str())
                    .collect();

                let error = ParseError::new(
                    format!("unknown condition `{name}`"),
                    start..end,
                    if defined.is_empty() {
                        String::from("expected `app` or `layout`")
                    } else {
                        format!("expected `app`, `layout` or one of: {}", defined.join(", "))
                    },
                );

                let candidates = PREDICATES.into_iter().chain(defined);
                Err(match closest(&name.to_lowercase(), candidates) {
                    Some(suggestion) => error.with_help(format!("did you mean `{suggestion}`?")),
                    None => error,
                })
            }
        }
    }

    /// `!name`, where `name` is a condition that `.when` defines
    fn parse_negated_check(&mut self) -> Result<Term, ParseError> {
        let start = self.pos;
        self.pos += NOT.len_utf8();
        self.skip_inline();

        let name_start = self.pos;
        let name = self.take_while(is_ident_continue);
        let span = if name.is_empty() {
            self.here()
        } else {
            name_start..self.pos
        };

        if name.is_empty() {
            return Err(ParseError::new(
                format!("expected the name of a condition after `{NOT}`, found {}", self.found()),
                span,
                "expected a name here",
            )
            .with_help(format!("`{NOT}docked` is the opposite of a condition that `{DEFINE_CHECK} docked : command` defines")));
        }

        if !self.defined.iter().any(|(defined, _)| defined == name) {
            let error = ParseError::new(
                format!("`{name}` is not a condition that is defined"),
                start..self.pos,
                format!("`{NOT}` is for the conditions that `{DEFINE_CHECK}` defines"),
            );

            let defined = self.defined.iter().map(|(defined, _)| defined.as_str());
            return Err(match closest(&name.to_lowercase(), defined) {
                Some(suggestion) => error.with_help(format!("did you mean `{NOT}{suggestion}`?")),
                None => error.with_help(format!(
                    "define it before the bindings with `{DEFINE_CHECK} {name} : command`"
                )),
            });
        }

        self.used.insert(name.to_string());
        Ok(Term::Predicate(Predicate::Check {
            name: name.to_string(),
            negated: true,
        }))
    }

    /// One or more values separated by `|`, each a run of words, with the span of each
    fn parse_values(&mut self) -> Result<Vec<(String, Range<usize>)>, ParseError> {
        let mut values = vec![];

        loop {
            self.skip_inline();
            let start = self.pos;
            let mut end = start;
            let mut words = vec![];

            while self.peek().is_some_and(is_value_char) {
                words.push(self.take_while(is_value_char));
                end = self.pos;
                self.skip_inline();
            }

            if words.is_empty() {
                return Err(ParseError::new(
                    format!("expected a value, found {}", self.found()),
                    self.here(),
                    "expected a value here",
                ));
            }

            values.push((words.join(" "), start..end));

            if !self.eat(OR) {
                break;
            }
        }

        Ok(values)
    }
}

/// The command on a line that continues on the next, without the `\`: a `\` that ends the line
/// after whitespace (or is all there is)
pub(crate) fn continuation(line: &str) -> Option<&str> {
    let text = line.strip_suffix('\\')?;
    (text.is_empty() || text.ends_with([' ', '\t'])).then(|| text.trim_end())
}

/// The entries of the blocks of a whkdrc, with the keys that each is for
type BlockBindings = Vec<(Vec<String>, Vec<HotkeyBinding>)>;

/// What the hotkey of a binding stands for: one set of keys, or if it has `{...}` groups, one for
/// each way of choosing from them
struct Hotkey {
    expansions: Vec<Vec<String>>,
    /// The groups, which the command of the binding may have a group for each of
    groups: Vec<braces::Group>,
    span: Range<usize>,
}

/// Where in `text`, the text between the groups of a hotkey, the `when` that guards it starts. A
/// `when` after a `+` is a key
fn guard_start(text: &str) -> Option<usize> {
    words(text).find_map(|(at, word)| {
        let before = text[..at].trim_end().chars().next_back();
        (word == WHEN && at > 0 && before != Some('+')).then_some(at)
    })
}

/// The words of `text` and where they start
fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = &text[at..];
        let skipped = rest.len()
            - rest
                .trim_start_matches(|c: char| !is_ident_continue(c))
                .len();
        let start = at + skipped;
        let word = text[start..]
            .split(|c: char| !is_ident_continue(c))
            .next()
            .filter(|word| !word.is_empty())?;
        at = start + word.len();
        Some((start, word))
    })
}

/// A mistake in a group, as the error to report, for a group that starts at `open` in the file
fn group_error(error: &braces::GroupError, open: usize) -> ParseError {
    let mut report = ParseError::new(
        error.message.clone(),
        error.span.start + open..error.span.end + open,
        error.label.clone(),
    );
    report.help = error.help.clone();
    report
}

enum Term {
    Default,
    Predicate(Predicate),
}

/// Something that is not a keyboard key but can trigger a binding: a mouse button or scroll
/// direction, or a swipe on a touchpad
#[derive(Debug, Copy, Clone)]
enum PointerInput {
    Mouse(MouseInput),
    Gesture(GestureInput),
}

impl PointerInput {
    fn parse(key: &str) -> Option<Self> {
        MouseInput::parse(key)
            .map(Self::Mouse)
            .or_else(|| GestureInput::parse(key).map(Self::Gesture))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Mouse(input) => input.name(),
            Self::Gesture(input) => input.name(),
        }
    }

    /// What to call it in a message
    fn kind(self) -> &'static str {
        match self {
            Self::Mouse(_) => "mouse input",
            Self::Gesture(_) => "gesture",
        }
    }

    /// Only the primary mouse buttons: swallowing every click would leave no way to fix the config
    fn requires_modifier(self) -> bool {
        match self {
            Self::Mouse(input) => input.requires_modifier(),
            Self::Gesture(_) => false,
        }
    }
}

/// A mouse input or a gesture triggers a binding, so it has to be the last key, and the primary
/// buttons need a modifier or every click would be swallowed
fn check_mouse_inputs(
    keys: &[String],
    spans: &[Range<usize>],
    allow_mouse: bool,
) -> Result<(), ParseError> {
    for (index, key) in keys.iter().enumerate() {
        let Some(input) = PointerInput::parse(key) else {
            continue;
        };
        let (name, kind) = (input.name(), input.kind());
        let span = spans[index].clone();

        if !allow_mouse {
            return Err(ParseError::new(
                format!("`{name}` is a {kind}, which cannot be used here"),
                span,
                "expected a keyboard key",
            )
            .with_help(
                "this hotkey is handled by the keyboard hook, so it has to be a keyboard key",
            ));
        }

        if index + 1 != keys.len() {
            return Err(ParseError::new(
                format!("{kind} `{name}` must be the last key"),
                span,
                "this is what triggers the binding",
            )
            .with_help(format!(
                "the keys before the last one are held down: `alt + {name}`"
            )));
        }

        if input.requires_modifier() && keys.len() == 1 {
            return Err(ParseError::new(
                format!("`{name}` needs a modifier"),
                span,
                "this would swallow every click",
            )
            .with_help(format!("bind it with a modifier, such as `alt + {name}`")));
        }
    }

    Ok(())
}

#[derive(Clone)]
enum EntryKind {
    /// A plain `keys : command` binding
    Plain,
    /// The `Default` line of a block
    Default,
    /// A `when` line, or any other line of a block
    Conditional(Condition),
}

/// One binding, as the checks in [`lint`] need to see it
struct Entry {
    /// The keys as the user wrote them, for messages
    display: String,
    /// The keys normalised, so that `Alt + H` and `alt + h` are the same key
    key: String,
    kind: EntryKind,
    span: Range<usize>,
}

impl Entry {
    fn new(keys: &[String], span: Range<usize>, kind: EntryKind) -> Self {
        let (trigger, mods) = keys.split_last().expect("hotkeys are never empty");
        let mut normalised: Vec<String> = mods.iter().map(|key| key.to_lowercase()).collect();
        normalised.sort();
        normalised.push(trigger.to_lowercase());

        Self {
            display: keys.join(" + "),
            key: normalised.join("+"),
            kind,
            span,
        }
    }
}

/// Things that parse fine but are probably not what was meant
fn lint(entries: &[Entry]) -> Vec<ParseError> {
    let mut groups: Vec<Vec<&Entry>> = vec![];
    for entry in entries {
        match groups.iter_mut().find(|group| group[0].key == entry.key) {
            Some(group) => group.push(entry),
            None => groups.push(vec![entry]),
        }
    }

    let warning = |message: String, span: &Range<usize>, label: &str| {
        ParseError::new(message, span.clone(), label).with_severity(Severity::Warning)
    };

    let mut found = vec![];

    for group in groups {
        let display = &group[0].display;

        let fallbacks: Vec<usize> = (0..group.len())
            .filter(|&i| matches!(group[i].kind, EntryKind::Plain | EntryKind::Default))
            .collect();

        // an explicit `Default` is preferred to a plain binding, then the first one wins
        let winner = fallbacks
            .iter()
            .copied()
            .find(|&i| matches!(group[i].kind, EntryKind::Default))
            .or_else(|| fallbacks.first().copied());

        if let Some(winner) = winner {
            for &i in fallbacks.iter().filter(|&&i| i != winner) {
                found.push(
                    warning(
                        format!("`{display}` already has a default command"),
                        &group[i].span,
                        "this one is never used",
                    )
                    .with_note(group[winner].span.clone(), "this one is used instead"),
                );
            }
        }

        let conditionals: Vec<(usize, &Condition)> = group
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| match &entry.kind {
                EntryKind::Conditional(condition) => Some((i, condition)),
                EntryKind::Plain | EntryKind::Default => None,
            })
            .collect();

        for (n, &(j, later)) in conditionals.iter().enumerate() {
            if let Some(&(i, _)) = conditionals[..n]
                .iter()
                .find(|(_, earlier)| covers(earlier, later))
            {
                found.push(
                    warning(
                        format!("this entry for `{display}` can never be reached"),
                        &group[j].span,
                        "an earlier entry always matches first",
                    )
                    .with_note(
                        group[i].span.clone(),
                        "this matches whenever the later one does",
                    ),
                );
            }
        }

        if winner.is_none() {
            if let Some(&(first, _)) = conditionals.first() {
                found.push(
                    ParseError::new(
                        format!("`{display}` has no default command"),
                        group[first].span.clone(),
                        "the key is swallowed when none of these conditions match",
                    )
                    .with_severity(Severity::Advice)
                    .with_help(
                        "add a plain `keys : command` binding, or a `Default` line (`Ignore` does nothing)",
                    ),
                );
            }
        }
    }

    found
}

/// Whether `a` matches whenever `b` does
/// A binding for an application that `.ignore` lists never runs
fn lint_ignored(entries: &[Entry], ignore: &[String]) -> Vec<ParseError> {
    let mut warnings = vec![];

    for entry in entries {
        let EntryKind::Conditional(Condition::Predicates(predicates)) = &entry.kind else {
            continue;
        };

        let ignored = predicates.iter().find_map(|predicate| match predicate {
            Predicate::App {
                names,
                negated: false,
            } if names.iter().all(|name| ignore.contains(name)) => Some(names),
            _ => None,
        });

        if let Some(names) = ignored {
            warnings.push(
                ParseError::new(
                    format!("this never runs, `{}` is ignored", names.join(" | ")),
                    entry.span.clone(),
                    "no binding runs in an application that is in `.ignore`",
                )
                .with_severity(Severity::Warning)
                .with_help("remove it from `.ignore`, or remove this entry"),
            );
        }
    }

    warnings
}

fn covers(a: &Condition, b: &Condition) -> bool {
    match (a, b) {
        (Condition::Predicates(a), Condition::Predicates(b)) => a
            .iter()
            .all(|needed| b.iter().any(|given| implies(given, needed))),
        _ => false,
    }
}

/// Whether `given` holding means `needed` holds
fn implies(given: &Predicate, needed: &Predicate) -> bool {
    match (given, needed) {
        (
            Predicate::App {
                names: given,
                negated: given_negated,
            },
            Predicate::App {
                names: needed,
                negated: needed_negated,
            },
        ) => set_implies(given, *given_negated, needed, *needed_negated),
        (
            Predicate::Layout {
                layouts: given,
                negated: given_negated,
            },
            Predicate::Layout {
                layouts: needed,
                negated: needed_negated,
            },
        ) => set_implies(given, *given_negated, needed, *needed_negated),
        (
            Predicate::Check {
                name: given,
                negated: given_negated,
            },
            Predicate::Check {
                name: needed,
                negated: needed_negated,
            },
        ) => given == needed && given_negated == needed_negated,
        (
            Predicate::CheckIs {
                name: given_name,
                values: given,
                negated: given_negated,
            },
            Predicate::CheckIs {
                name: needed_name,
                values: needed,
                negated: needed_negated,
            },
        ) => {
            given_name == needed_name && set_implies(given, *given_negated, needed, *needed_negated)
        }
        _ => false,
    }
}

/// `x in given` (or `x not in given`) implies `x in needed` (or `x not in needed`)
fn set_implies<T: PartialEq>(
    given: &[T],
    given_negated: bool,
    needed: &[T],
    needed_negated: bool,
) -> bool {
    match (given_negated, needed_negated) {
        (false, false) => given.iter().all(|x| needed.contains(x)),
        (true, true) => needed.iter().all(|x| given.contains(x)),
        (false, true) => given.iter().all(|x| !needed.contains(x)),
        (true, false) => false,
    }
}

fn unknown_layout(value: &str, span: Range<usize>) -> ParseError {
    let error = ParseError::new(
        format!("unknown layout `{value}`"),
        span,
        "not a built-in komorebi layout",
    );

    match closest(
        &value.to_lowercase(),
        Layout::ALL.into_iter().map(Layout::name),
    ) {
        Some(suggestion) => error.with_help(format!("did you mean `{suggestion}`?")),
        None => error.with_help(format!(
            "valid layouts: {}",
            Layout::ALL.map(Layout::name).join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_line_parse() {
        let src = r#"
.shell pwsh # can be one of cmd | pwsh | powershell

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from("echo \"Hello\""),
                condition: Condition::Always,
            }],
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_starts_with_comment_single_line_parser() {
        let src = r#"
# sample comment
.shell pwsh # can be one of cmd | pwsh | powershell

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from("echo \"Hello\""),
                condition: Condition::Always,
            }],
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_parse() {
        let src = r#"
.shell cmd

# Specify different behaviour depending on the app
alt + n [
    # ProcessName as shown by `Get-Process`
    Firefox       : echo "hello firefox"

    # Spaces are fine, no quotes required
    Google Chrome : echo "hello chrome"
]

# leading newlines are fine
# line comments should parse and be ignored
alt + h     : komorebic focus left # so should comments at the end of a line
alt + j     : komorebic focus down
alt + k     : komorebic focus up
alt + l     : komorebic focus right

# so should empty lines
alt + 1 : komorebic focus-workspace 0 # digits are fine in the hotkeys section

# trailing newlines are fine


"#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Cmd,
            app_bindings: vec![(
                vec![String::from("alt"), String::from("n")],
                vec![
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello firefox""#),
                        condition: Condition::app("Firefox"),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        condition: Condition::app("Google Chrome"),
                    },
                ],
            )],
            bindings: vec![
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("h")],
                    command: String::from("komorebic focus left"),
                    condition: Condition::Always,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("j")],
                    command: String::from("komorebic focus down"),
                    condition: Condition::Always,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("k")],
                    command: String::from("komorebic focus up"),
                    condition: Condition::Always,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("l")],
                    command: String::from("komorebic focus right"),
                    condition: Condition::Always,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("1")],
                    command: String::from("komorebic focus-workspace 0"),
                    condition: Condition::Always,
                },
            ],
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_binding_without_modkeys() {
        let src = r#"
# sample comment
.shell pwsh # can be one of cmd | pwsh | powershell

f11 : echo "hello f11""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("f11")],
                command: String::from("echo \"hello f11\""),
                condition: Condition::Always,
            }],
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_default_and_scoped_ignores() {
        let src = r#"
.shell pwsh

alt + n [
    Default       : echo "hello world"
    Firefox       : echo "hello firefox"
    Google Chrome : echo "hello chrome"
    Zen Browser   : Ignore
]

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![(
                vec![String::from("alt"), String::from("n")],
                vec![
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello world""#),
                        condition: Condition::Default,
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello firefox""#),
                        condition: Condition::app("Firefox"),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        condition: Condition::app("Google Chrome"),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from("Ignore"),
                        condition: Condition::app("Zen Browser"),
                    },
                ],
            )],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                condition: Condition::Always,
            }],
            pause_binding: None,
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hotkey() {
        let src = r#"
.shell pwsh
.pause ctrl + shift + esc

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                condition: Condition::Always,
            }],
            pause_binding: Some(vec![
                "ctrl".to_string(),
                "shift".to_string(),
                "esc".to_string(),
            ]),
            pause_hook: None,
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hook() {
        let src = r#"
.shell pwsh
.pause ctrl + shift + esc
.pause_hook komorebic toggle-pause

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                condition: Condition::Always,
            }],
            pause_binding: Some(vec![
                "ctrl".to_string(),
                "shift".to_string(),
                "esc".to_string(),
            ]),
            pause_hook: Some("komorebic toggle-pause".to_string()),
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    #[test]
    fn test_pause_hook_with_comments() {
        let src = r#"
.shell pwsh                            # can be one of cmd | pwsh | powershell
.pause alt + shift + p                 # can be any hotkey combo to toggle all other hotkeys on and off
.pause_hook komorebic toggle-pause     # another comment

alt + h : echo "Hello""#;

        let output = parse(src);
        let expected = Whkdrc {
            shell: Shell::Pwsh,
            app_bindings: vec![],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                condition: Condition::Always,
            }],
            pause_binding: Some(vec![
                "alt".to_string(),
                "shift".to_string(),
                "p".to_string(),
            ]),
            pause_hook: Some("komorebic toggle-pause".to_string()),
            wheel_cooldown: None,
            gesture_cooldown: None,
            ignore: vec![],
            checks: vec![],
        };

        assert_eq!(output.unwrap(), expected);
    }

    fn err(src: &str) -> ParseError {
        parse(src).unwrap_err()
    }

    #[test]
    fn test_crlf() {
        let src = ".shell cmd\r\n.pause alt + p # c\r\n\r\nalt + n [\r\n  Firefox : a\r\n]\r\nalt + h : echo hi # x\r\n";
        let output = parse(src).unwrap();
        assert_eq!(output.bindings[0].command, "echo hi");
        assert_eq!(output.app_bindings[0].1[0].command, "a");
        assert_eq!(output.pause_binding, Some(vec!["alt".into(), "p".into()]));
    }

    #[test]
    fn test_bom() {
        let output = parse("\u{feff}.shell cmd\nf1 : x").unwrap();
        assert_eq!(output.shell, Shell::Cmd);
    }

    #[test]
    fn test_process_name_starting_with_default() {
        let src = ".shell cmd\nalt + n [\n DefaultApp : a\n Default : b\n]\nf1 : x";
        let output = parse(src).unwrap();
        let conditions: Vec<_> = output.app_bindings[0]
            .1
            .iter()
            .map(|b| b.condition.clone())
            .collect();
        assert_eq!(
            conditions,
            vec![Condition::app("DefaultApp"), Condition::Default]
        );
    }

    #[test]
    fn test_digits_only_key() {
        let output = parse(".shell cmd\n1 : x").unwrap();
        assert_eq!(output.bindings[0].keys, vec!["1"]);
    }

    #[test]
    fn test_error_unknown_shell() {
        let src = ".shell zsh\nf1 : x";
        let e = err(src);
        assert_eq!(&src[e.span.clone()], "zsh");
        assert_eq!(e.message, "unknown shell `zsh`");
    }

    #[test]
    fn test_error_missing_shell() {
        assert_eq!(err("f1 : x").message, "missing `.shell` directive");
    }

    #[test]
    fn test_error_missing_colon() {
        let src = ".shell cmd\nalt + h echo\n";
        let e = err(src);
        assert_eq!(&src[e.span.clone()], "e");
        assert!(e.message.starts_with("expected `:`, `[` or `when`"));
    }

    #[test]
    fn test_error_unterminated_bracket() {
        let src = ".shell cmd\nalt + n [\n Firefox : a\n";
        let e = err(src);
        assert_eq!(e.message, "unterminated `[`");
        assert_eq!(&src[e.notes[0].0.clone()], "[");
    }

    #[test]
    fn test_error_no_bindings() {
        assert_eq!(
            err(".shell cmd\n# nothing\n").message,
            "expected at least one hotkey binding"
        );
        assert!(parse("").is_err());
    }

    #[test]
    fn test_error_directive_after_binding() {
        assert!(err(".shell cmd\nf1 : x\n.pause f2\n").help.is_some());
    }

    #[test]
    fn test_error_non_ascii_span() {
        let src = ".shell cmd\nf1 : é\nf2 ✓\n";
        let e = err(src);
        assert_eq!(&src[e.span.clone()], "✓");
    }

    #[test]
    fn test_report_rendering() {
        let src = ".shell zsh\nf1 : x\n";
        let rendered = err(src).report("whkdrc", src, false);
        assert!(rendered.contains("unknown shell `zsh`"), "{rendered}");
        assert!(rendered.contains("whkdrc:1:8"), "{rendered}");
        assert!(rendered.contains("cmd | pwsh | powershell"), "{rendered}");
    }

    fn validator() -> KeyValidator {
        let known = ["alt", "ctrl", "escape", "h", "j", "shift", "space"];
        KeyValidator::new(
            move |name| known.contains(&name.to_lowercase().as_str()) || name.parse::<u8>().is_ok(),
            known.map(String::from).to_vec(),
        )
    }

    #[test]
    fn test_unknown_key_suggests_closest() {
        let src = ".shell cmd
alt + hh : x";
        let e = parse_with(src, &validator()).unwrap_err();
        assert_eq!(e.message, "unknown key `hh`");
        assert_eq!(&src[e.span.clone()], "hh");
        assert_eq!(e.help.as_deref(), Some("did you mean `h`?"));

        let e = parse_with(
            ".shell cmd
shfit + h : x",
            &validator(),
        )
        .unwrap_err();
        assert_eq!(e.help.as_deref(), Some("did you mean `shift`?"));
    }

    #[test]
    fn test_unknown_key_without_suggestion() {
        let e = parse_with(
            ".shell cmd
alt + qqqqqq : x",
            &validator(),
        )
        .unwrap_err();
        assert_eq!(e.message, "unknown key `qqqqqq`");
        assert!(e.help.is_none());
    }

    #[test]
    fn test_unknown_key_in_pause_binding() {
        let src = ".shell cmd
.pause ctlr + p
h : x";
        let e = parse_with(src, &validator()).unwrap_err();
        assert_eq!(&src[e.span.clone()], "ctlr");
        assert_eq!(e.help.as_deref(), Some("did you mean `ctrl`?"));
    }

    #[test]
    fn test_valid_keys_pass_validation() {
        assert!(parse_with(
            ".shell cmd
Ctrl + 1 : x
alt + h : y",
            &validator()
        )
        .is_ok());
    }

    #[test]
    fn test_synonym_suggestion() {
        let keys = validator().with_synonyms(&[("enter", "escape"), ("del", "nonexistent")]);

        let e = parse_with(
            ".shell cmd
Enter : x",
            &keys,
        )
        .unwrap_err();
        assert_eq!(e.help.as_deref(), Some("did you mean `escape`?"));

        // synonyms whose target is not a valid key are dropped
        let e = parse_with(
            ".shell cmd
del : x",
            &keys,
        )
        .unwrap_err();
        assert_ne!(e.help.as_deref(), Some("did you mean `nonexistent`?"));
    }

    fn block(entries: &str) -> String {
        format!(".shell cmd\nalt + n [\n{entries}\n]\nf1 : x")
    }

    fn conditions(entries: &str) -> Vec<Condition> {
        parse(&block(entries)).unwrap().app_bindings[0]
            .1
            .iter()
            .map(|b| b.condition.clone())
            .collect()
    }

    fn layout(layouts: &[Layout], negated: bool) -> Predicate {
        Predicate::Layout {
            layouts: layouts.to_vec(),
            negated,
        }
    }

    fn app(names: &[&str], negated: bool) -> Predicate {
        Predicate::App {
            names: names.iter().map(ToString::to_string).collect(),
            negated,
        }
    }

    #[test]
    fn test_app_forms_are_equivalent() {
        assert_eq!(
            conditions("Firefox : a\napp = Firefox : b\napp=Firefox:c"),
            vec![Condition::app("Firefox"); 3]
        );
    }

    #[test]
    fn test_app_alternatives_and_spaces() {
        assert_eq!(
            conditions("app = Google Chrome | Zen Browser : a"),
            vec![Condition::Predicates(vec![app(
                &["Google Chrome", "Zen Browser"],
                false
            )])]
        );
    }

    #[test]
    fn test_layout_conditions() {
        assert_eq!(
            conditions(
                "layout = bsp : a\nlayout = bsp | grid : b\nlayout != rows : c\nlayout = UltrawideVerticalStack : d"
            ),
            vec![
                Condition::Predicates(vec![layout(&[Layout::Bsp], false)]),
                Condition::Predicates(vec![layout(&[Layout::Bsp, Layout::Grid], false)]),
                Condition::Predicates(vec![layout(&[Layout::Rows], true)]),
                Condition::Predicates(vec![layout(&[Layout::UltrawideVerticalStack], false)]),
            ]
        );
    }

    #[test]
    fn test_combined_conditions() {
        assert_eq!(
            conditions("Firefox & layout = rows : a\napp != Code & layout != bsp : b"),
            vec![
                Condition::Predicates(vec![
                    app(&["Firefox"], false),
                    layout(&[Layout::Rows], false)
                ]),
                Condition::Predicates(vec![app(&["Code"], true), layout(&[Layout::Bsp], true)]),
            ]
        );
    }

    #[test]
    fn test_bare_process_named_like_a_predicate() {
        assert_eq!(
            conditions("layout : a\napp : b"),
            vec![Condition::app("layout"), Condition::app("app")]
        );
    }

    #[test]
    fn test_default_may_come_first_in_the_file() {
        assert_eq!(
            conditions("Default : a\nlayout = bsp : b"),
            vec![
                Condition::Default,
                Condition::Predicates(vec![layout(&[Layout::Bsp], false)])
            ]
        );
    }

    #[test]
    fn test_error_unknown_layout() {
        let src = block("layout = bsp | bpsp : a");
        let e = parse(&src).unwrap_err();
        assert_eq!(e.message, "unknown layout `bpsp`");
        assert_eq!(&src[e.span.clone()], "bpsp");
        assert_eq!(e.help.as_deref(), Some("did you mean `bsp`?"));
    }

    #[test]
    fn test_error_unknown_layout_without_suggestion() {
        let e = parse(&block("layout = zzzzzzzz : a")).unwrap_err();
        assert!(e.help.unwrap().starts_with("valid layouts: bsp, columns"));
    }

    #[test]
    fn test_error_unknown_condition() {
        let src = block("layot = bsp : a");
        let e = parse(&src).unwrap_err();
        assert_eq!(e.message, "unknown condition `layot`");
        assert_eq!(&src[e.span.clone()], "layot");
        assert_eq!(e.help.as_deref(), Some("did you mean `layout`?"));
    }

    #[test]
    fn test_error_default_combined() {
        let e = parse(&block("Default & layout = bsp : a")).unwrap_err();
        assert_eq!(
            e.message,
            "`Default` cannot be combined with other conditions"
        );
    }

    #[test]
    fn test_error_missing_value() {
        let e = parse(&block("layout = : a")).unwrap_err();
        assert!(e.message.starts_with("expected a value"));
    }

    fn warnings(src: &str) -> Vec<ParseError> {
        parse_full(src, None).unwrap().warnings
    }

    fn messages(src: &str) -> Vec<String> {
        warnings(src).into_iter().map(|w| w.message).collect()
    }

    #[test]
    fn test_blocks_and_plain_bindings_in_any_order() {
        let src =
            ".shell cmd\nf1 : a\nalt + n [\n Firefox : b\n]\nf2 : c\nalt + m [\n Code : d\n]\n";
        let output = parse(src).unwrap();
        assert_eq!(output.bindings.len(), 2);
        assert_eq!(output.app_bindings.len(), 2);
    }

    #[test]
    fn test_when_guard() {
        let src = ".shell cmd\nalt + left : stack left\nalt + left when layout = scrolling : consume left\n";
        let output = parse(src).unwrap();

        assert_eq!(output.bindings.len(), 1);
        assert_eq!(output.bindings[0].condition, Condition::Always);

        let (keys, entries) = &output.app_bindings[0];
        assert_eq!(keys, &vec![String::from("alt"), String::from("left")]);
        assert_eq!(entries[0].command, "consume left");
        assert_eq!(
            entries[0].condition,
            Condition::Predicates(vec![layout(&[Layout::Scrolling], false)])
        );
    }

    #[test]
    fn test_when_composes_like_blocks() {
        let src = ".shell cmd\nf1 : x\nf1 when app = Firefox | Zen Browser & layout != bsp | grid : y\nf1 when Code : z\n";
        let output = parse(src).unwrap();
        let conditions: Vec<_> = output
            .app_bindings
            .iter()
            .map(|(_, e)| e[0].condition.clone())
            .collect();

        assert_eq!(
            conditions,
            vec![
                Condition::Predicates(vec![
                    app(&["Firefox", "Zen Browser"], false),
                    layout(&[Layout::Bsp, Layout::Grid], true),
                ]),
                Condition::app("Code"),
            ]
        );
    }

    #[test]
    fn test_when_errors() {
        let e = err(".shell cmd\nf1 : x\nf1 when Default : y\n");
        assert_eq!(e.message, "`Default` cannot be used after `when`");

        let e = err(".shell cmd\nf1 when layout = bpsp : y\n");
        assert_eq!(e.help.as_deref(), Some("did you mean `bsp`?"));

        // values may span words (for `app = Google Chrome`), so this reads `bsp y` as one layout
        let e = err(".shell cmd\nf1 when layout = bsp y\n");
        assert_eq!(e.message, "unknown layout `bsp y`");

        let e = err(".shell cmd\nf1 when app = Firefox\n");
        assert!(e.message.starts_with("expected `:`"));
    }

    #[test]
    fn test_a_key_named_when_is_not_a_guard() {
        let output = parse(".shell cmd\nalt + when : x\n").unwrap();
        assert_eq!(output.bindings[0].keys, vec!["alt", "when"]);
    }

    #[test]
    fn test_empty_command_is_accepted_with_a_warning() {
        let src = ".shell cmd\nf1 :\nf2 : x\n";
        let output = parse(src).unwrap();
        assert_eq!(output.bindings[0].command, "");
        assert_eq!(messages(src), vec!["empty command"]);
    }

    #[test]
    fn test_no_warnings_for_a_typical_config() {
        let src = ".shell cmd\nalt + n [\n Firefox : a\n Default : b\n]\nf1 : x\nf1 when layout = bsp : y\nf1 when layout = grid : z\n";
        assert!(messages(src).is_empty());
    }

    #[test]
    fn test_warn_on_a_second_default() {
        let src = ".shell cmd\nalt + n [\n Default : a\n]\nalt + n : b\n";
        let w = warnings(src);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].message, "`alt + n` already has a default command");
        assert_eq!(&src[w[0].span.clone()], "alt + n");
        // the explicit `Default` is the one that is used
        assert_eq!(&src[w[0].notes[0].0.clone()], "Default");
    }

    #[test]
    fn test_warn_on_duplicate_plain_bindings_in_any_spelling() {
        let src = ".shell cmd\nAlt + H : a\nshift + alt + h : b\nalt + h : c\n";
        // named as first written, and `shift + alt + h` is a different hotkey
        assert_eq!(
            messages(src),
            vec!["`Alt + H` already has a default command"]
        );

        // the modifiers can be written in any order
        let src = ".shell cmd\nctrl + alt + h : a\nalt + ctrl + h : b\n";
        assert_eq!(messages(src).len(), 1);
    }

    #[test]
    fn test_warn_on_unreachable_entries() {
        let src = ".shell cmd\nf1 : x\nf1 when layout = bsp | grid : a\nf1 when layout = bsp : b\n";
        assert_eq!(
            messages(src),
            vec!["this entry for `f1` can never be reached"]
        );

        // a narrower entry first is fine
        let src = ".shell cmd\nf1 : x\nf1 when layout = bsp : a\nf1 when layout = bsp | grid : b\n";
        assert!(messages(src).is_empty());

        // an identical condition written differently
        let src = ".shell cmd\nf1 : x\nf1 when Firefox : a\nf1 when app = Firefox : b\n";
        assert_eq!(messages(src).len(), 1);

        // `!=` is covered by a wider `!=`, and by a disjoint `=`
        let src = ".shell cmd\nf1 : x\nf1 when layout != bsp : a\nf1 when layout = grid : b\n";
        assert_eq!(messages(src).len(), 1);

        // unrelated predicates never shadow each other
        let src = ".shell cmd\nf1 : x\nf1 when app = Code : a\nf1 when layout = bsp : b\n";
        assert!(messages(src).is_empty());
    }

    #[test]
    fn test_advice_when_a_key_has_no_default() {
        let src = ".shell cmd\nf2 : y\nalt + n [\n Firefox : a\n]\n";
        let w = warnings(src);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].severity, Severity::Advice);
        assert_eq!(w[0].message, "`alt + n` has no default command");
        assert_eq!(&src[w[0].span.clone()], "Firefox");
    }

    #[test]
    fn test_warnings_render() {
        let src = ".shell cmd\nf1 : x\nf1 : y\n";
        let w = warnings(src);
        let rendered = w[0].report("whkdrc", src, false);
        assert!(rendered.contains("Warning"), "{rendered}");
        assert!(rendered.contains("this one is used instead"), "{rendered}");
    }

    #[test]
    fn test_mouse_inputs_are_triggers() {
        let src = ".shell cmd\nalt + wheelup : a\nAlt + Shift + MButton : b\nalt + xbutton1 when layout = scrolling : c\nalt + wheeldown [\n Firefox : d\n Default : e\n]\nwheelleft : f\n";
        let output = parse(src).unwrap();
        assert_eq!(output.bindings[0].keys, vec!["alt", "wheelup"]);
        assert_eq!(output.bindings[1].keys, vec!["Alt", "Shift", "MButton"]);
        assert_eq!(output.app_bindings.len(), 2);
        // not primary buttons, so no modifier is needed
        assert_eq!(output.bindings[2].keys, vec!["wheelleft"]);
    }

    #[test]
    fn test_error_mouse_input_must_be_last() {
        let src = ".shell cmd\nwheelup + alt : x\n";
        let e = err(src);
        assert_eq!(e.message, "mouse input `wheelup` must be the last key");
        assert_eq!(&src[e.span.clone()], "wheelup");

        let e = err(".shell cmd\nalt + lbutton + h : x\n");
        assert_eq!(e.message, "mouse input `lbutton` must be the last key");
    }

    #[test]
    fn test_error_primary_buttons_need_a_modifier() {
        for button in ["lbutton", "RButton"] {
            let src = format!(".shell cmd\n{button} : x\n");
            let e = err(&src);
            assert!(e.message.ends_with("needs a modifier"), "{}", e.message);
            assert_eq!(&src[e.span.clone()], button);
            assert!(e.help.unwrap().contains("alt +"));

            // in a block, and in a guard, too
            assert!(parse(&format!(
                ".shell cmd\nf1 : y\n{button} [\n Firefox : x\n]\n"
            ))
            .is_err());
            assert!(parse(&format!(".shell cmd\nf1 : y\n{button} when Firefox : x\n")).is_err());
        }

        // with a modifier they are fine
        assert!(parse(".shell cmd\nalt + lbutton : x\n").is_ok());
    }

    #[test]
    fn test_error_pause_cannot_be_a_mouse_input() {
        let src = ".shell cmd\n.pause alt + mbutton\nf1 : x\n";
        let e = err(src);
        assert!(e.message.contains("mouse input"), "{}", e.message);
        assert_eq!(&src[e.span.clone()], "mbutton");
    }

    #[test]
    fn test_mouse_input_lints_see_through_spelling() {
        // the same hotkey written two ways is still a duplicate default
        let src = ".shell cmd\nAlt + WheelUp : a\nalt + wheelup : b\n";
        assert_eq!(
            messages(src),
            vec!["`Alt + WheelUp` already has a default command"]
        );
    }

    #[test]
    fn test_only_modifiers_can_be_held_with_a_mouse_input() {
        let keys = KeyValidator::new(|_| true, vec![])
            .with_modifiers(|key| ["alt", "ctrl"].contains(&key));

        let src = ".shell cmd
alt + h + wheelup : x
";
        let e = parse_with(src, &keys).unwrap_err();
        assert_eq!(e.message, "`h` cannot be held down with `wheelup`");
        assert_eq!(&src[e.span.clone()], "h");

        // modifiers are fine, and so is every keyboard binding
        assert!(parse_with(
            ".shell cmd
ctrl + alt + wheelup : x
",
            &keys
        )
        .is_ok());
        assert!(parse_with(
            ".shell cmd
alt + h : x
",
            &keys
        )
        .is_ok());
        // without the knowledge of which keys are modifiers, nothing is checked
        assert!(parse(
            ".shell cmd
alt + h + wheelup : x
"
        )
        .is_ok());
    }

    #[test]
    fn test_wheel_cooldown() {
        let with = |directive: &str| parse(&format!(".shell cmd\n{directive}\nf1 : x\n"));

        assert_eq!(parse(".shell cmd\nf1 : x\n").unwrap().wheel_cooldown, None);
        assert_eq!(
            with(".wheel_cooldown 150").unwrap().wheel_cooldown,
            Some(150)
        );
        assert_eq!(
            with(".wheel_cooldown 0 # off").unwrap().wheel_cooldown,
            Some(0)
        );
        assert_eq!(
            with(".wheel_cooldown   60000   ").unwrap().wheel_cooldown,
            Some(60_000)
        );

        // after the other directives
        let src = ".shell cmd\n.pause alt + p\n.pause_hook x\n.wheel_cooldown 20\nf1 : y\n";
        let output = parse(src).unwrap();
        assert_eq!(output.wheel_cooldown, Some(20));
        assert_eq!(output.pause_hook.as_deref(), Some("x"));
    }

    #[test]
    fn test_error_wheel_cooldown() {
        let src = ".shell cmd\n.wheel_cooldown fast\nf1 : x\n";
        let e = err(src);
        assert_eq!(e.message, "expected a number of milliseconds, found `fast`");
        assert_eq!(&src[e.span.clone()], "fast");

        for bad in ["", "150ms", "-5", "+5", "1.5", "99999999999999999999"] {
            let src = format!(".shell cmd\n.wheel_cooldown {bad}\nf1 : x\n");
            assert!(parse(&src).is_err(), "{bad:?}");
        }

        let e = err(".shell cmd\n.wheel_cooldown\nf1 : x\n");
        assert!(e
            .message
            .starts_with("expected a number of milliseconds, found end of line"));

        let e = err(".shell cmd\n.wheel_cooldown 60001\nf1 : x\n");
        assert_eq!(e.message, "a cooldown of 60001 milliseconds is too long");

        // once, and in its place among the directives
        assert!(parse(".shell cmd\n.wheel_cooldown 1\n.wheel_cooldown 2\nf1 : x\n").is_err());
        assert!(parse(".shell cmd\n.wheel_cooldown 1\n.pause alt + p\nf1 : x\n").is_err());
    }

    #[test]
    fn test_gestures_are_triggers() {
        let src = ".shell cmd\nswipe3left : a\nAlt + Swipe4Up : b\nalt + swipe3down when layout = scrolling : c\nswipe3right [\n Firefox : d\n Default : e\n]\n";
        let output = parse(src).unwrap();
        assert_eq!(output.bindings[0].keys, vec!["swipe3left"]);
        assert_eq!(output.bindings[1].keys, vec!["Alt", "Swipe4Up"]);
        assert_eq!(output.app_bindings.len(), 2);
    }

    #[test]
    fn test_error_gesture_must_be_last() {
        let src = ".shell cmd\nswipe3left + alt : x\n";
        let e = err(src);
        assert_eq!(e.message, "gesture `swipe3left` must be the last key");
        assert_eq!(&src[e.span.clone()], "swipe3left");
    }

    #[test]
    fn test_error_pause_cannot_be_a_gesture() {
        let src = ".shell cmd\n.pause alt + swipe4up\nf1 : x\n";
        let e = err(src);
        assert_eq!(
            e.message,
            "`swipe4up` is a gesture, which cannot be used here"
        );
        assert_eq!(&src[e.span.clone()], "swipe4up");
    }

    #[test]
    fn test_only_modifiers_can_be_held_with_a_gesture() {
        let keys = KeyValidator::new(|_| true, vec![]).with_modifiers(|key| key == "alt");

        let src = ".shell cmd\nalt + h + swipe3up : x\n";
        let e = parse_with(src, &keys).unwrap_err();
        assert_eq!(e.message, "`h` cannot be held down with `swipe3up`");
        assert_eq!(
            e.help.as_deref(),
            Some("only alt, ctrl, shift and win can be used with a gesture")
        );
        assert!(parse_with(".shell cmd\nalt + swipe3up : x\n", &keys).is_ok());
    }

    #[test]
    fn test_gestures_do_not_need_a_modifier() {
        // unlike the primary mouse buttons, they cannot lock anyone out
        for gesture in whkd_core::GestureInput::ALL {
            let src = format!(".shell cmd\n{gesture} : x\n");
            assert!(parse(&src).is_ok(), "{gesture}");
        }
    }

    #[test]
    fn test_gesture_cooldown() {
        let with = |directives: &str| parse(&format!(".shell cmd\n{directives}\nf1 : x\n"));

        assert_eq!(
            parse(".shell cmd\nf1 : x\n").unwrap().gesture_cooldown,
            None
        );
        assert_eq!(
            with(".gesture_cooldown 400 # ms").unwrap().gesture_cooldown,
            Some(400)
        );
        assert_eq!(
            with(".gesture_cooldown 0").unwrap().gesture_cooldown,
            Some(0)
        );

        // either, or both, in that order, after the other directives
        let both = with(".pause alt + p\n.wheel_cooldown 20\n.gesture_cooldown 30").unwrap();
        assert_eq!(
            (both.wheel_cooldown, both.gesture_cooldown),
            (Some(20), Some(30))
        );
        assert_eq!(with(".wheel_cooldown 20").unwrap().gesture_cooldown, None);
        assert_eq!(with(".gesture_cooldown 30").unwrap().wheel_cooldown, None);

        assert!(with(".gesture_cooldown 30\n.wheel_cooldown 20").is_err());
        assert!(with(".gesture_cooldown 30\n.gesture_cooldown 40").is_err());
        assert!(with(".gesture_cooldown soon").is_err());
        assert!(with(".gesture_cooldown 60001").is_err());
    }

    fn commands(src: &str) -> Vec<String> {
        let parsed = parse(src).unwrap();
        parsed
            .app_bindings
            .iter()
            .flat_map(|(_, bindings)| bindings)
            .chain(&parsed.bindings)
            .map(|binding| binding.command.clone())
            .collect()
    }

    #[test]
    fn test_continued_command() {
        assert_eq!(
            commands(".shell cmd\nf1 : echo one \\\n    two \\\n  three\nf2 : next\n"),
            ["echo one two three", "next"]
        );
    }

    #[test]
    fn test_command_may_start_on_the_next_line() {
        assert_eq!(commands(".shell cmd\nf1 : \\\n    echo hi\n"), ["echo hi"]);
    }

    #[test]
    fn test_backslash_ending_a_word_is_part_of_the_command() {
        // paths end in a backslash, and always could
        let src = ".shell cmd\nf1 : explorer C:\\Users\\\nf2 : echo \\\\\nf3 : x\n";
        assert_eq!(commands(src), ["explorer C:\\Users\\", "echo \\\\", "x"]);
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn test_backslash_before_a_comment_is_not_a_continuation() {
        let src = ".shell cmd\nf1 : echo hi \\ # a comment\nf2 : next\n";
        assert_eq!(commands(src), ["echo hi \\", "next"]);
    }

    #[test]
    fn test_a_comment_ends_a_continued_command() {
        let src = ".shell cmd\nf1 : echo one \\\n  two # done\nf2 : next\n";
        assert_eq!(commands(src), ["echo one two", "next"]);
    }

    #[test]
    fn test_continuation_in_blocks_guards_and_pause_hook() {
        let src = ".shell cmd\n.pause_hook echo a \\\n  b\nf1 [\n  Code : echo c \\\n    d\n  Default : e\n]\nf2 when layout = bsp : echo f \\\n  g\nf3 : h\n";
        let parsed = parse(src).unwrap();
        assert_eq!(parsed.pause_hook.as_deref(), Some("echo a b"));
        assert_eq!(commands(src), ["echo c d", "e", "echo f g", "h"]);
    }

    #[test]
    fn test_continuation_with_crlf() {
        let src = ".shell cmd\r\nf1 : echo one \\\r\n  two\r\nf2 : x\r\n";
        assert_eq!(commands(src), ["echo one two", "x"]);
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn test_continuation_onto_nothing_is_flagged() {
        for (src, what) in [
            (".shell cmd\nf1 : echo one \\\n\nf2 : x\n", "a blank line"),
            (
                ".shell cmd\nf1 : echo one \\\n# note\nf2 : x\n",
                "a comment",
            ),
            (
                ".shell cmd\nf2 : x\nf1 : echo one \\\n",
                "the end of the file",
            ),
            (
                ".shell cmd\nf2 : x\nf1 : echo one \\",
                "the end without a newline",
            ),
        ] {
            let found = warnings(src);
            assert_eq!(found.len(), 1, "{what}: {found:?}");
            assert_eq!(found[0].message, "nothing to continue onto", "{what}");
            assert_eq!(&src[found[0].span.clone()], "\\", "{what}");
            assert!(commands(src).contains(&"echo one".to_string()), "{what}");
        }
    }

    #[test]
    fn test_errors_after_a_continued_command_point_at_the_right_line() {
        let src = ".shell cmd\nf1 : echo one \\\n  two\nf2 nonsense\n";
        let error = err(src);
        assert_eq!(&src[error.span.clone()], "n");
        assert!(
            error.report("whkdrc", src, false).contains("whkdrc:4:4"),
            "{}",
            error.report("whkdrc", src, false)
        );
    }

    fn bindings(src: &str) -> Vec<(Vec<String>, String)> {
        let parsed = parse(src).unwrap();
        parsed
            .app_bindings
            .iter()
            .flat_map(|(_, bindings)| bindings)
            .chain(&parsed.bindings)
            .map(|binding| (binding.keys.clone(), binding.command.clone()))
            .collect()
    }

    fn keys_and_commands(src: &str) -> Vec<String> {
        bindings(src)
            .into_iter()
            .map(|(keys, command)| format!("{} : {command}", keys.join(" + ")))
            .collect()
    }

    #[test]
    fn test_braces_make_a_binding_for_each_element() {
        assert_eq!(
            keys_and_commands(
                ".shell cmd\nalt + {h,j,k,l} : komorebic focus {left,down,up,right}\n"
            ),
            [
                "alt + h : komorebic focus left",
                "alt + j : komorebic focus down",
                "alt + k : komorebic focus up",
                "alt + l : komorebic focus right",
            ]
        );
    }

    #[test]
    fn test_an_underscore_leaves_a_key_out_and_the_first_group_changes_the_slowest() {
        assert_eq!(
            keys_and_commands(
                ".shell cmd\nalt + {_,shift +} {h,j} : komorebic {focus,move} {left,down}\n"
            ),
            [
                "alt + h : komorebic focus left",
                "alt + j : komorebic focus down",
                "alt + shift + h : komorebic move left",
                "alt + shift + j : komorebic move down",
            ]
        );
    }

    #[test]
    fn test_ranges() {
        assert_eq!(
            keys_and_commands(".shell cmd\nalt + {1-3} : focus-workspace {0-2}\n"),
            [
                "alt + 1 : focus-workspace 0",
                "alt + 2 : focus-workspace 1",
                "alt + 3 : focus-workspace 2",
            ]
        );

        assert_eq!(
            keys_and_commands(".shell cmd\nalt + {a-c,x} : {a-c,x}\n").len(),
            4
        );
    }

    #[test]
    fn test_a_group_can_be_the_whole_hotkey_or_all_but_a_modifier() {
        assert_eq!(
            keys_and_commands(".shell cmd\n{f1,f2} : echo {one,two}\n"),
            ["f1 : echo one", "f2 : echo two"]
        );
        assert_eq!(
            keys_and_commands(".shell cmd\n{alt,ctrl} + h : echo\n"),
            ["alt + h : echo", "ctrl + h : echo"]
        );
        assert_eq!(
            keys_and_commands(".shell cmd\n{ alt , ctrl + shift }+h : x\n"),
            ["alt + h : x", "ctrl + shift + h : x"]
        );
    }

    #[test]
    fn test_a_command_may_have_fewer_groups_than_the_hotkey_or_none() {
        assert_eq!(
            keys_and_commands(".shell cmd\n{alt,ctrl} + {h,j} : echo {1,2}\n"),
            [
                "alt + h : echo 1",
                "alt + j : echo 1",
                "ctrl + h : echo 2",
                "ctrl + j : echo 2",
            ]
        );
        assert_eq!(
            keys_and_commands(".shell cmd\nalt + {h,j} : echo same\n"),
            ["alt + h : echo same", "alt + j : echo same"]
        );
    }

    #[test]
    fn test_braces_in_the_command_of_a_hotkey_without_groups_are_untouched() {
        // a script block is a command like any other, and always was
        let src = ".shell pwsh\nalt + h : Get-Process | ForEach-Object { $_.Name } {a,b}\n";
        assert_eq!(
            keys_and_commands(src),
            ["alt + h : Get-Process | ForEach-Object { $_.Name } {a,b}"]
        );
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn test_a_backslash_makes_a_brace_part_of_the_command() {
        assert_eq!(
            keys_and_commands(".shell pwsh\nalt + {h,j} : Get-Process | % \\{ $_.Name \\} {a,b}\n"),
            [
                "alt + h : Get-Process | % { $_.Name } a",
                "alt + j : Get-Process | % { $_.Name } b",
            ]
        );
    }

    #[test]
    fn test_guards_and_blocks_are_expanded_too() {
        let parsed = parse(
            ".shell cmd\nalt + {h,j} when layout = bsp : echo {a,b}\nalt + {k,l} [\n  Firefox : echo {c,d}\n  Default : echo {e,f}\n]\nalt + m : plain\n",
        )
        .unwrap();

        assert_eq!(parsed.bindings.len(), 1);

        let described: Vec<(String, Vec<(String, Condition)>)> = parsed
            .app_bindings
            .iter()
            .map(|(keys, bindings)| {
                (
                    keys.join(" + "),
                    bindings
                        .iter()
                        .map(|binding| (binding.command.clone(), binding.condition.clone()))
                        .collect(),
                )
            })
            .collect();

        let bsp = Condition::Predicates(vec![Predicate::Layout {
            layouts: vec![Layout::Bsp],
            negated: false,
        }]);
        let firefox = Condition::Predicates(vec![Predicate::App {
            names: vec![String::from("Firefox")],
            negated: false,
        }]);

        assert_eq!(
            described,
            vec![
                (
                    String::from("alt + h"),
                    vec![(String::from("echo a"), bsp.clone())]
                ),
                (String::from("alt + j"), vec![(String::from("echo b"), bsp)]),
                (
                    String::from("alt + k"),
                    vec![
                        (String::from("echo c"), firefox.clone()),
                        (String::from("echo e"), Condition::Default),
                    ]
                ),
                (
                    String::from("alt + l"),
                    vec![
                        (String::from("echo d"), firefox),
                        (String::from("echo f"), Condition::Default),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn test_a_key_group_before_a_guard_is_not_confused_with_it() {
        let parsed = parse(".shell cmd\n{a,b} when Firefox : x\n").unwrap();
        assert_eq!(parsed.app_bindings.len(), 2);

        // a key named `when` is a key, after a `+`
        let parsed = parse(".shell cmd\n{a,b} + when : x\n").unwrap();
        assert_eq!(parsed.bindings[1].keys, ["b", "when"]);
    }

    #[test]
    fn test_a_continued_command_is_expanded_as_a_whole() {
        assert_eq!(
            keys_and_commands(".shell cmd\n{alt,ctrl} + {h,j} : echo {a,b} \\\n    {c,d}\n"),
            [
                "alt + h : echo a c",
                "alt + j : echo a d",
                "ctrl + h : echo b c",
                "ctrl + j : echo b d",
            ]
        );
    }

    #[test]
    fn test_an_empty_command_is_flagged_once() {
        let src = ".shell cmd\nalt + {h,j} :\nf1 : x\n";
        assert_eq!(bindings(src).len(), 3);
        assert_eq!(messages(src), ["empty command"]);
    }

    #[test]
    fn test_mouse_inputs_and_gestures_can_be_elements() {
        let parsed = parse(
            ".shell cmd\nalt + {wheelup,wheeldown} : echo {up,down}\n{swipe3left,swipe3right} : x\n",
        )
        .unwrap();
        assert_eq!(parsed.bindings.len(), 4);
        assert_eq!(parsed.bindings[1].keys, ["alt", "wheeldown"]);
    }

    #[test]
    fn test_a_binding_is_a_duplicate_if_an_expansion_is() {
        let src = ".shell cmd\nalt + h : first\nalt + {h,j} : second\n";
        let found = warnings(src);
        assert_eq!(found.len(), 1, "{found:?}");
        // reported where the hotkey is written
        assert_eq!(&src[found[0].span.clone()], "alt + {h,j}");
    }

    #[test]
    fn test_directives_keep_their_own_syntax() {
        let error = err(".shell cmd\n.pause {a,b}\nf1 : x\n");
        assert!(
            error.message.contains("expected a key name"),
            "{}",
            error.message
        );
    }

    #[test]
    fn test_errors_after_a_binding_with_groups_are_on_the_right_line() {
        let src = ".shell cmd\nalt + {h,j} : x {a,b}\nf2 nonsense\n";
        let error = err(src);
        assert!(
            error.report("whkdrc", src, false).contains("whkdrc:3:4"),
            "{}",
            error.report("whkdrc", src, false)
        );
    }

    fn group_error(src: &str) -> ParseError {
        err(&format!(".shell cmd\n{src}\n"))
    }

    #[test]
    fn test_mistakes_in_a_group_are_where_it_is() {
        let src = ".shell cmd\nalt + {h,j,k : x\n";
        let error = group_error("alt + {h,j,k : x");
        assert_eq!(error.message, "unterminated `{`");
        assert_eq!(&src[error.span.clone()], "{");

        let src = ".shell cmd\nalt + {h,{j,k}} : x\n";
        let error = group_error("alt + {h,{j,k}} : x");
        assert_eq!(error.message, "groups cannot be nested");
        assert_eq!(&src[error.span.clone()], "{");
        assert_eq!(error.span.start, src.find("{j").unwrap());

        let src = ".shell cmd\nalt + {h,,k} : x\n";
        let error = group_error("alt + {h,,k} : x");
        assert_eq!(error.message, "empty element");
        assert_eq!(
            error.help.as_deref(),
            Some("write `_` for an element that is nothing")
        );
        assert_eq!(error.span.start, src.find(",,").unwrap() + 1);

        let src = ".shell cmd\nalt + {9-1} : x\n";
        let error = group_error("alt + {9-1} : x");
        assert_eq!(error.message, "invalid range `9-1`");
        assert_eq!(&src[error.span.clone()], "9-1");
    }

    #[test]
    fn test_too_many_bindings_are_refused() {
        let error = group_error("alt + {1-9}{1-9}{a-z}{a-z} : x");
        assert!(error.message.contains("more than 512"), "{}", error.message);
        assert_eq!(error.label, "54756 bindings");

        let src = format!(".shell cmd\nalt + {} : x\n", "{a-z}".repeat(40));
        assert_eq!(err(&src).label, "far too many");
    }

    #[test]
    fn test_a_command_group_of_another_length_is_a_mistake_with_a_note() {
        let src = ".shell cmd\nalt + {h,j} : echo {a,b,c}\n";
        let error = err(src);
        assert_eq!(
            error.message,
            "this group has 3 elements, but the group of the hotkey it goes with has 2"
        );
        assert_eq!(&src[error.span.clone()], "{a,b,c}");
        assert_eq!(error.notes.len(), 1);
        assert_eq!(&src[error.notes[0].0.clone()], "{h,j}");
    }

    #[test]
    fn test_a_mistake_in_a_continued_command_is_found_as_a_whole() {
        // the lines were joined, so where in them it is, is not where it is in the file
        let src = ".shell cmd\nalt + {h,j} : echo \\\n    {a,b,c}\n";
        let error = err(src);
        assert!(error.message.contains("3 elements"), "{}", error.message);
        assert!(
            src[error.span.clone()].starts_with("echo"),
            "{:?}",
            &src[error.span.clone()]
        );
    }

    #[test]
    fn test_a_command_with_more_groups_than_the_hotkey_is_a_mistake() {
        let error = err(".shell cmd\nalt + {h,j} : echo {a,b} {c,d}\n");
        assert_eq!(
            error.message,
            "this command has 2 groups, and the hotkey has 1"
        );
        assert!(error.help.unwrap().contains(r"\{"));
    }

    #[test]
    fn test_a_mistake_in_a_block_command_is_found() {
        let error = err(".shell cmd\nalt + {h,j} [\n  Firefox : echo {a}\n]\n");
        assert!(error.message.contains("1 elements"), "{}", error.message);
    }

    #[test]
    fn test_an_unknown_key_in_a_group_points_at_the_element() {
        let src = ".shell cmd\nalt + {h,jj,k} : x\n";
        let error = parse_with(src, &validator()).unwrap_err();
        assert_eq!(error.message, "unknown key `jj`");
        assert_eq!(&src[error.span.clone()], "jj");
        // with what it stands for, and the suggestion for the key
        assert_eq!(error.notes.len(), 1);
        assert_eq!(&src[error.notes[0].0.clone()], "alt + {h,jj,k}");
        assert_eq!(error.notes[0].1, "when this stands for `alt + jj`");

        let src = ".shell cmd\nalt + {h,spac} : x\n";
        let error = parse_with(src, &validator()).unwrap_err();
        assert_eq!(error.message, "unknown key `spac`");
        assert_eq!(error.help.as_deref(), Some("did you mean `space`?"));
    }

    #[test]
    fn test_a_mistake_in_the_text_around_a_group_is_found_there() {
        let src = ".shell cmd\nalt + hh{a,b} : x\n";
        let error = parse_with(src, &validator()).unwrap_err();
        assert_eq!(error.message, "unknown key `hha`");

        let src = ".shell cmd\nalt + {a,b}wheelup : x\n";
        assert!(parse_with(src, &validator()).is_err());
    }

    #[test]
    fn test_keys_that_are_not_joined_are_a_mistake() {
        let src = ".shell cmd\nalt {h,j} : x\n";
        let error = err(src);
        assert_eq!(error.message, "expected `+` between keys, found `h`");

        // the mistake is that there is no `+`, even where the group has the key
        let error = err(".shell cmd\nalt + {_,shift} h : x\n");
        assert_eq!(error.message, "expected `+` between keys, found `h`");
    }

    #[test]
    fn test_unicode_in_groups_and_commands() {
        assert_eq!(
            keys_and_commands(".shell cmd\nf{1,2} : echo é{ß,ü}\n"),
            ["f1 : echo éß", "f2 : echo éü"]
        );
    }

    #[test]
    fn test_ignore_directive() {
        let with = |directives: &str| parse(&format!(".shell cmd\n{directives}\nf1 : x\n"));

        assert!(parse(".shell cmd\nf1 : x\n").unwrap().ignore.is_empty());
        assert_eq!(with(".ignore Firefox").unwrap().ignore, ["Firefox"]);
        assert_eq!(
            with(".ignore Firefox | Google Chrome|  Zen Browser # note")
                .unwrap()
                .ignore,
            ["Firefox", "Google Chrome", "Zen Browser"]
        );
        assert_eq!(
            with(".ignore my-app.v2 | code_oss").unwrap().ignore,
            ["my-app.v2", "code_oss"]
        );

        // after the other directives, which it does not need
        let all = with(
            ".pause alt + p\n.pause_hook echo x\n.wheel_cooldown 20\n.gesture_cooldown 30\n.ignore Code",
        )
        .unwrap();
        assert_eq!(all.ignore, ["Code"]);
        assert_eq!(all.gesture_cooldown, Some(30));
        assert_eq!(with(".wheel_cooldown 20").unwrap().ignore.len(), 0);
    }

    #[test]
    fn test_ignore_directive_goes_last() {
        for src in [
            ".shell cmd\n.ignore Code\n.gesture_cooldown 30\nf1 : x\n",
            ".shell cmd\n.ignore Code\n.pause alt + p\nf1 : x\n",
            ".shell cmd\nf1 : x\n.ignore Code\n",
            ".shell cmd\n.ignore Code\n.ignore Firefox\nf1 : x\n",
        ] {
            let error = err(src);
            assert!(
                error.message.starts_with("expected a key name"),
                "{src:?}: {}",
                error.message
            );
            // and the order is spelt out
            assert!(error.help.unwrap().contains("`.ignore`"), "{src:?}");
        }
    }

    #[test]
    fn test_ignore_directive_needs_an_application() {
        for src in [
            ".shell cmd\n.ignore\nf1 : x\n",
            ".shell cmd\n.ignore # none\nf1 : x\n",
            ".shell cmd\n.ignore Code |\nf1 : x\n",
        ] {
            let error = err(src);
            assert!(
                error.message.starts_with("expected a value"),
                "{src:?}: {}",
                error.message
            );
            assert!(
                error
                    .help
                    .as_deref()
                    .unwrap()
                    .contains(".ignore Firefox | Google Chrome"),
                "{src:?}"
            );
        }
    }

    /// What is said about entries that never run, and nothing else
    fn never_runs(src: &str) -> Vec<ParseError> {
        warnings(src)
            .into_iter()
            .filter(|warning| warning.message.starts_with("this never runs"))
            .collect()
    }

    #[test]
    fn test_ignored_applications_do_not_have_bindings_that_run() {
        let src = ".shell cmd\n.ignore Firefox | Code\nf1 [\n  Firefox : a\n  Notepad : b\n  Default : c\n]\nf2 when app = Code : d\nf3 [\n  app = Firefox | Notepad : e\n  app = Code & layout = bsp : f\n  app != Firefox : g\n  Ignore Me : h\n]\n";
        let found = never_runs(src);

        // the entries that are only for ignored applications, and not those that are for one as well
        let flagged: Vec<&str> = found.iter().map(|w| &src[w.span.clone()]).collect();
        assert_eq!(
            flagged,
            ["Firefox", "f2", "app = Code & layout = bsp"],
            "{found:?}"
        );
        assert_eq!(found[0].message, "this never runs, `Firefox` is ignored");
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(
            found[0].help.as_deref(),
            Some("remove it from `.ignore`, or remove this entry")
        );
    }

    #[test]
    fn test_nothing_is_flagged_without_an_ignored_application() {
        let src = ".shell cmd\n.ignore Code\nf1 [\n  Firefox : a\n  Default : b\n]\nf2 : c\nf3 when app != Code : d\n";
        assert!(never_runs(src).is_empty());

        // an application that is not in the list is not ignored for being a part of one
        let src = ".shell cmd\n.ignore Fire\nf1 [\n  Firefox : a\n]\n";
        assert!(never_runs(src).is_empty());

        // and without the directive there is nothing to be ignored
        let src = ".shell cmd\nf1 [\n  Firefox : a\n]\n";
        assert!(never_runs(src).is_empty());
    }

    fn check_predicate(name: &str, negated: bool) -> Predicate {
        Predicate::Check {
            name: name.to_string(),
            negated,
        }
    }

    #[test]
    fn test_conditions_are_defined_with_when() {
        let parsed = parse(
            ".shell cmd\n.when docked : ping -n 1 -w 100 10.0.0.1 >nul # the dock\n.when wide : powershell -c \"exit 1\"\nf1 when docked & wide : x\n",
        )
        .unwrap();

        assert_eq!(
            parsed.checks,
            [
                whkd_core::Check {
                    name: String::from("docked"),
                    command: String::from("ping -n 1 -w 100 10.0.0.1 >nul"),
                },
                whkd_core::Check {
                    name: String::from("wide"),
                    command: String::from("powershell -c \"exit 1\""),
                },
            ]
        );
        assert!(parse(".shell cmd\nf1 : x\n").unwrap().checks.is_empty());
    }

    #[test]
    fn test_a_definition_can_be_continued_and_comes_after_the_other_directives() {
        let parsed = parse(
            ".shell cmd\n.pause alt + p\n.wheel_cooldown 20\n.ignore Code\n.when docked : echo one \\\n    two\nf1 when docked : x\n",
        )
        .unwrap();
        assert_eq!(parsed.checks[0].command, "echo one two");
        assert_eq!(parsed.ignore, ["Code"]);

        // not before them
        for src in [
            ".shell cmd\n.when docked : x\n.ignore Code\nf1 when docked : x\n",
            ".shell cmd\n.when docked : x\n.pause alt + p\nf1 when docked : x\n",
            ".shell cmd\nf1 when x : x\n.when docked : x\n",
        ] {
            let error = err(src);
            assert!(
                error.message.starts_with("expected"),
                "{src:?}: {}",
                error.message
            );
        }
    }

    #[test]
    fn test_a_condition_is_used_by_its_name_in_guards_and_blocks() {
        let parsed = parse(
            ".shell cmd\n.when docked : x\nf1 when docked : a\nf2 when !docked & layout = bsp : b\nf3 [\n  docked : c\n  !docked : d\n  Default : e\n]\n",
        )
        .unwrap();

        let conditions: Vec<Condition> = parsed
            .app_bindings
            .iter()
            .flat_map(|(_, bindings)| bindings.iter().map(|binding| binding.condition.clone()))
            .collect();

        assert_eq!(
            conditions,
            vec![
                Condition::Predicates(vec![check_predicate("docked", false)]),
                Condition::Predicates(vec![
                    check_predicate("docked", true),
                    Predicate::Layout {
                        layouts: vec![Layout::Bsp],
                        negated: false
                    },
                ]),
                Condition::Predicates(vec![check_predicate("docked", false)]),
                Condition::Predicates(vec![check_predicate("docked", true)]),
                Condition::Default,
            ]
        );
    }

    #[test]
    fn test_a_name_that_is_not_defined_is_a_process_as_it_always_was() {
        let parsed = parse(".shell cmd\nf1 when docked : a\nf2 [\n  docked : b\n]\n").unwrap();

        let app = Condition::app("docked");
        assert!(parsed
            .app_bindings
            .iter()
            .all(|(_, bindings)| bindings[0].condition == app));
    }

    #[test]
    fn test_a_definition_is_what_its_name_means_and_app_is_for_the_process() {
        let parsed = parse(
            ".shell cmd\n.when Firefox : x\nf1 when Firefox : a\nf2 when app = Firefox : b\n",
        )
        .unwrap();

        assert_eq!(
            parsed.app_bindings[0].1[0].condition,
            Condition::Predicates(vec![check_predicate("Firefox", false)])
        );
        assert_eq!(
            parsed.app_bindings[1].1[0].condition,
            Condition::app("Firefox")
        );
    }

    #[test]
    fn test_names_are_case_sensitive_and_only_whole_names_count() {
        let parsed =
            parse(".shell cmd\n.when docked : x\nf1 when Docked : a\nf2 when docked2 : b\nf3 when docked : c\n")
                .unwrap();

        assert_eq!(
            parsed.app_bindings[0].1[0].condition,
            Condition::app("Docked")
        );
        assert_eq!(
            parsed.app_bindings[1].1[0].condition,
            Condition::app("docked2")
        );
        assert_eq!(
            parsed.app_bindings[2].1[0].condition,
            Condition::Predicates(vec![check_predicate("docked", false)])
        );
    }

    #[test]
    fn test_not_equals_is_not_a_negation() {
        let parsed =
            parse(".shell cmd\n.when docked : x\nf1 when app != Code & docked : a\n").unwrap();

        assert_eq!(
            parsed.app_bindings[0].1[0].condition,
            Condition::Predicates(vec![
                Predicate::App {
                    names: vec![String::from("Code")],
                    negated: true
                },
                check_predicate("docked", false),
            ])
        );
    }

    #[test]
    fn test_a_definition_needs_a_name_a_colon_and_a_command() {
        for (src, message) in [
            (
                ".shell cmd\n.when\nf1 : x\n",
                "expected a name after `.when`, found end of line",
            ),
            (
                ".shell cmd\n.when : x\nf1 : x\n",
                "expected a name after `.when`, found `:`",
            ),
            (
                ".shell cmd\n.when 1x : x\nf1 : x\n",
                "`1x` cannot be the name of a condition",
            ),
            (
                ".shell cmd\n.when docked x\nf1 : x\n",
                "expected `:`, found `x`",
            ),
            (
                ".shell cmd\n.when docked\nf1 : x\n",
                "expected `:`, found end of line",
            ),
            (
                ".shell cmd\n.when docked :\nf1 : x\n",
                "`docked` has no command",
            ),
            (
                ".shell cmd\n.when docked : # nothing\nf1 : x\n",
                "`docked` has no command",
            ),
        ] {
            let error = err(src);
            assert_eq!(error.message, message, "{src:?}");
            assert!(error.help.is_some(), "{src:?}");
        }
    }

    #[test]
    fn test_names_that_mean_something_are_refused() {
        for name in ["Default", "Ignore", "app", "layout", "when"] {
            let error = err(&format!(".shell cmd\n.when {name} : x\nf1 : x\n"));
            assert_eq!(
                error.message,
                format!("`{name}` cannot be the name of a condition")
            );
            assert_eq!(error.label, "it already means something in a condition");
        }
    }

    #[test]
    fn test_a_condition_is_defined_once() {
        let src = ".shell cmd\n.when docked : x\n.when docked : y\nf1 when docked : z\n";
        let error = err(src);
        assert_eq!(error.message, "`docked` is already defined");
        assert_eq!(&src[error.span.clone()], "docked");
        assert_eq!(error.span.start, src.rfind("docked : y").unwrap());
        assert_eq!(&src[error.notes[0].0.clone()], "docked");
        assert_eq!(error.notes[0].0.start, src.find("docked").unwrap());
    }

    #[test]
    fn test_the_opposite_of_what_is_not_defined_is_a_mistake_with_a_suggestion() {
        let src = ".shell cmd\n.when docked : x\nf1 when !dokced : a\n";
        let error = err(src);
        assert_eq!(error.message, "`dokced` is not a condition that is defined");
        assert_eq!(&src[error.span.clone()], "!dokced");
        assert_eq!(error.help.as_deref(), Some("did you mean `!docked`?"));

        let error = err(".shell cmd\nf1 when !nothing : a\n");
        assert_eq!(
            error.help.as_deref(),
            Some("define it before the bindings with `.when nothing : command`")
        );
    }

    #[test]
    fn test_a_bang_needs_a_name() {
        let error = err(".shell cmd\n.when docked : x\nf1 when ! : a\n");
        assert!(
            error
                .message
                .starts_with("expected the name of a condition after `!`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn test_a_condition_that_is_not_used_is_flagged() {
        let src = ".shell cmd\n.when docked : x\n.when wide : y\nf1 when docked : a\n";
        let found: Vec<ParseError> = warnings(src)
            .into_iter()
            .filter(|warning| warning.message.contains("never used"))
            .collect();

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].message, "`wide` is defined but never used");
        assert_eq!(&src[found[0].span.clone()], "wide");
        assert_eq!(
            found[0].help.as_deref(),
            Some("use it as `when wide`, or remove it")
        );
        assert_eq!(found[0].severity, Severity::Warning);
    }

    #[test]
    fn test_the_opposite_counts_as_using_a_condition() {
        let src = ".shell cmd\n.when docked : x\nf1 when !docked : a\nf1 when docked : b\n";
        assert!(warnings(src)
            .iter()
            .all(|warning| !warning.message.contains("never used")));

        let src = ".shell cmd\n.when docked : x\nf1 when !docked : a\n";
        assert!(warnings(src)
            .iter()
            .all(|warning| !warning.message.contains("never used")));
    }

    #[test]
    fn test_entries_that_can_never_be_reached_are_found_for_conditions_too() {
        // the same condition, which is the same answer
        let src = ".shell cmd\n.when docked : x\nf1 [\n  docked : a\n  docked & layout = bsp : b\n  Default : c\n]\n";
        let found = messages(src);
        assert!(
            found
                .iter()
                .any(|message| message.contains("can never be reached")),
            "{found:?}"
        );

        // and the opposite is not
        let src =
            ".shell cmd\n.when docked : x\nf1 [\n  docked : a\n  !docked : b\n  Default : c\n]\n";
        assert!(!messages(src)
            .iter()
            .any(|message| message.contains("can never be reached")));
    }

    #[test]
    fn test_conditions_work_with_groups_of_braces() {
        let parsed =
            parse(".shell cmd\n.when docked : x\nalt + {h,j} when docked : echo {a,b}\n").unwrap();
        assert_eq!(parsed.app_bindings.len(), 2);
        assert_eq!(
            parsed.app_bindings[1].1[0].condition,
            Condition::Predicates(vec![check_predicate("docked", false)])
        );
    }

    fn check_is(name: &str, values: &[&str], negated: bool) -> Predicate {
        Predicate::CheckIs {
            name: name.to_string(),
            values: values.iter().map(ToString::to_string).collect(),
            negated,
        }
    }

    #[test]
    fn test_a_condition_can_be_compared_with_what_it_writes() {
        let parsed = parse(
            ".shell cmd\n.when mode : mode.exe\nf1 when mode = work : a\nf2 when mode != work | home & layout = bsp : b\nf3 [\n  mode = Google Chrome : c\n  mode != x : d\n  mode : e\n  Default : f\n]\n",
        )
        .unwrap();

        let conditions: Vec<Condition> = parsed
            .app_bindings
            .iter()
            .flat_map(|(_, bindings)| bindings.iter().map(|binding| binding.condition.clone()))
            .collect();

        assert_eq!(
            conditions,
            vec![
                Condition::Predicates(vec![check_is("mode", &["work"], false)]),
                Condition::Predicates(vec![
                    check_is("mode", &["work", "home"], true),
                    Predicate::Layout {
                        layouts: vec![Layout::Bsp],
                        negated: false
                    },
                ]),
                Condition::Predicates(vec![check_is("mode", &["Google Chrome"], false)]),
                Condition::Predicates(vec![check_is("mode", &["x"], true)]),
                Condition::Predicates(vec![check_predicate("mode", false)]),
                Condition::Default,
            ]
        );
    }

    #[test]
    fn test_a_comparison_counts_as_using_a_condition() {
        let src = ".shell cmd\n.when mode : x\nf1 when mode = work : a\n";
        assert!(warnings(src)
            .iter()
            .all(|warning| !warning.message.contains("never used")));
    }

    #[test]
    fn test_comparing_what_is_not_defined_is_an_unknown_condition_that_names_what_is() {
        let src = ".shell cmd\n.when mode : x\n.when docked : y\nf1 when mod = work : a\nf2 when docked : b\n";
        let error = err(src);
        assert_eq!(error.message, "unknown condition `mod`");
        assert_eq!(&src[error.span.clone()], "mod");
        assert_eq!(
            error.label,
            "expected `app`, `layout` or one of: mode, docked"
        );
        assert_eq!(error.help.as_deref(), Some("did you mean `mode`?"));

        // and with nothing defined it is as it was
        let error = err(".shell cmd\nf1 when mode = work : a\n");
        assert_eq!(error.label, "expected `app` or `layout`");
    }

    #[test]
    fn test_a_comparison_needs_a_value() {
        let error = err(".shell cmd\n.when mode : x\nf1 when mode = : a\n");
        assert!(
            error.message.starts_with("expected a value"),
            "{}",
            error.message
        );
    }

    #[test]
    fn test_entries_that_can_never_be_reached_are_found_for_comparisons_too() {
        let src = ".shell cmd\n.when mode : x\nf1 [\n  mode = work | home : a\n  mode = work : b\n  Default : c\n]\n";
        assert!(messages(src)
            .iter()
            .any(|message| message.contains("can never be reached")));

        let src = ".shell cmd\n.when mode : x\nf1 [\n  mode = work : a\n  mode = home : b\n  Default : c\n]\n";
        assert!(!messages(src)
            .iter()
            .any(|message| message.contains("can never be reached")));

        // another condition is another answer
        let src = ".shell cmd\n.when mode : x\n.when size : y\nf1 [\n  mode = work : a\n  size = work : b\n  Default : c\n]\n";
        assert!(!messages(src)
            .iter()
            .any(|message| message.contains("can never be reached")));
    }
}
