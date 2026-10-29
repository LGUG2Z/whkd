use ariadne::Config;
use ariadne::IndexType;
use ariadne::Label;
use ariadne::Report;
use ariadne::ReportKind;
use ariadne::Source;
use std::io::IsTerminal;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;
use whkd_core::Condition;
use whkd_core::HotkeyBinding;
use whkd_core::Layout;
use whkd_core::Predicate;
use whkd_core::Shell;
use whkd_core::Whkdrc;

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
    }
    .parse_file()
}

/// Knows which key names are valid, so the parser can point at bad ones in the whkdrc
pub struct KeyValidator {
    is_valid: Box<dyn Fn(&str) -> bool>,
    known: Vec<String>,
    synonyms: Vec<(String, String)>,
}

impl KeyValidator {
    /// `is_valid` is the source of truth; `known` is only used to suggest corrections
    pub fn new(is_valid: impl Fn(&str) -> bool + 'static, known: Vec<String>) -> Self {
        Self {
            is_valid: Box::new(is_valid),
            known,
            synonyms: vec![],
        }
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

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Cursor<'a> {
    src: &'a str,
    pos: usize,
    keys: Option<&'a KeyValidator>,
    /// Every binding seen, in file order, for the checks that run once parsing is done
    entries: Vec<Entry>,
    warnings: Vec<ParseError>,
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
            if self.peek() == Some('#') {
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

        let pause_binding = if self.at_keyword(".pause") {
            self.pos += ".pause".len();
            self.skip_inline();
            let keys = self.parse_hotkeys()?;
            self.skip_trivia();
            Some(keys)
        } else {
            None
        };

        let pause_hook = if self.at_keyword(".pause_hook") {
            self.pos += ".pause_hook".len();
            let command = self.parse_command();
            self.skip_trivia();
            Some(command)
        } else {
            None
        };

        let mut app_bindings = vec![];
        let mut bindings = vec![];

        while self.peek().is_some() {
            let start = self.pos;
            let keys = self.parse_hotkeys()?;
            let keys_span = start..self.trimmed_end();
            self.skip_inline();

            if self.peek() == Some('[') {
                let mapped = self.parse_app_binding(&keys)?;
                app_bindings.push((keys, mapped));
            } else if self.peek() == Some(':') {
                self.pos += 1;
                let command = self.parse_command();
                self.entries
                    .push(Entry::new(&keys, keys_span, EntryKind::Plain));
                bindings.push(HotkeyBinding {
                    keys,
                    command,
                    condition: Condition::Always,
                });
            } else if self.at_keyword("when") {
                self.pos += "when".len();
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
                self.expect(':')?;
                let command = self.parse_command();

                self.entries.push(Entry::new(
                    &keys,
                    keys_span,
                    EntryKind::Conditional(condition.clone()),
                ));
                app_bindings.push((
                    keys.clone(),
                    vec![HotkeyBinding {
                        keys,
                        command,
                        condition,
                    }],
                ));
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
        warnings.sort_by_key(|warning| warning.span.start);

        Ok(Parsed {
            whkdrc: Whkdrc {
                shell,
                app_bindings,
                bindings,
                pause_binding,
                pause_hook,
            },
            warnings,
        })
    }

    fn parse_shell(&mut self) -> Result<Shell, ParseError> {
        if !self.at_keyword(".shell") {
            return Err(ParseError::new(
                "missing `.shell` directive",
                self.here(),
                "expected `.shell` here",
            )
            .with_help("start your whkdrc with e.g. `.shell pwsh`"));
        }

        self.pos += ".shell".len();
        self.skip_inline();

        let start = self.pos;
        match self.take_while(is_ident_continue) {
            "pwsh" => Ok(Shell::Pwsh),
            "powershell" => Ok(Shell::Powershell),
            "cmd" => Ok(Shell::Cmd),
            other => {
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
                    "expected one of: cmd | pwsh | powershell",
                ))
            }
        }
    }

    fn parse_hotkeys(&mut self) -> Result<Vec<String>, ParseError> {
        let mut keys = vec![self.parse_key()?];

        loop {
            self.skip_inline();
            if !self.eat('+') {
                break;
            }

            self.skip_inline();
            keys.push(self.parse_key()?);
        }

        Ok(keys)
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
                    error.with_help(
                        "directives (`.shell`, `.pause`, `.pause_hook`) must come first, in that order",
                    )
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
    fn parse_command(&mut self) -> String {
        self.skip_inline();

        let command = self.take_while(|c| c != '\n' && c != '#').trim_end();
        if command.is_empty() {
            let span = self.here();
            self.warnings.push(
                ParseError::new("empty command", span, "there is nothing to run here")
                    .with_severity(Severity::Warning)
                    .with_help("add a command, or remove the binding"),
            );
        }

        command.to_string()
    }

    fn parse_app_binding(&mut self, keys: &[String]) -> Result<Vec<HotkeyBinding>, ParseError> {
        let open = self.pos;
        self.pos += 1;

        let mut mapped = vec![];

        loop {
            self.skip_trivia();

            if self.eat(']') {
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
            self.expect(':').map_err(|error| {
                error.with_note(open..open + 1, "inside this `[` block; is a `]` missing?")
            })?;
            let command = self.parse_command();

            let kind = match &condition {
                Condition::Default => EntryKind::Default,
                condition => EntryKind::Conditional(condition.clone()),
            };
            self.entries.push(Entry::new(keys, condition_span, kind));

            mapped.push(HotkeyBinding {
                keys: keys.to_vec(),
                command,
                condition,
            });
        }

        if mapped.is_empty() {
            return Err(ParseError::new(
                "empty app binding",
                open..self.pos,
                "expected at least one `condition : command` entry",
            ));
        }

        Ok(mapped)
    }

    /// `Default`, or one or more terms joined by `&`
    fn parse_condition(&mut self) -> Result<Condition, ParseError> {
        let start = self.pos;
        let mut terms = vec![self.parse_term()?];

        loop {
            self.skip_inline();
            if !self.eat('&') {
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

        let negated = if self.rest().starts_with("!=") {
            self.pos += 2;
            true
        } else if self.eat('=') {
            false
        } else {
            return Ok(if name == "Default" {
                Term::Default
            } else {
                Term::Predicate(Predicate::App {
                    names: vec![name],
                    negated: false,
                })
            });
        };

        let values = self.parse_values()?;

        match name.as_str() {
            "app" => Ok(Term::Predicate(Predicate::App {
                names: values.into_iter().map(|(value, _)| value).collect(),
                negated,
            })),
            "layout" => {
                let layouts = values
                    .into_iter()
                    .map(|(value, span)| {
                        Layout::parse(&value).ok_or_else(|| unknown_layout(&value, span))
                    })
                    .collect::<Result<_, _>>()?;

                Ok(Term::Predicate(Predicate::Layout { layouts, negated }))
            }
            _ => {
                let error = ParseError::new(
                    format!("unknown condition `{name}`"),
                    start..end,
                    "expected `app` or `layout`",
                );

                Err(match closest(&name.to_lowercase(), ["app", "layout"]) {
                    Some(suggestion) => error.with_help(format!("did you mean `{suggestion}`?")),
                    None => error,
                })
            }
        }
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

            if !self.eat('|') {
                break;
            }
        }

        Ok(values)
    }
}

enum Term {
    Default,
    Predicate(Predicate),
}

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

fn is_value_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
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
}
