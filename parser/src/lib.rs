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
use whkd_core::HotkeyBinding;
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

/// A syntax error in a whkdrc, with byte-offset spans into the source
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Range<usize>,
    pub label: String,
    pub notes: Vec<(Range<usize>, String)>,
    pub help: Option<String>,
}

impl ParseError {
    fn new(message: impl Into<String>, span: Range<usize>, label: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
            label: label.into(),
            notes: vec![],
            help: None,
        }
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

        let mut builder = Report::build(ReportKind::Error, span.clone())
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

/// Parse the contents of a whkdrc, without checking that key names are real keys
pub fn parse(src: &str) -> Result<Whkdrc, ParseError> {
    Cursor {
        src,
        pos: 0,
        keys: None,
    }
    .parse_file()
}

/// Parse the contents of a whkdrc, reporting unknown key names with a suggestion where possible
pub fn parse_with(src: &str, keys: &KeyValidator) -> Result<Whkdrc, ParseError> {
    Cursor {
        src,
        pos: 0,
        keys: Some(keys),
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

        let max = (key.chars().count() / 3).max(1);

        self.known
            .iter()
            .map(|name| (edit_distance(&key, name), name))
            .filter(|(distance, _)| *distance <= max)
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, name)| name.as_str())
    }
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

    fn found(&self) -> String {
        match self.peek() {
            None => String::from("end of file"),
            Some('\n' | '\r') => String::from("end of line"),
            Some(c) => format!("`{c}`"),
        }
    }

    fn at_directive(&self, name: &str) -> bool {
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

    fn parse_file(&mut self) -> Result<Whkdrc, ParseError> {
        self.skip_trivia();
        let shell = self.parse_shell()?;
        self.skip_trivia();

        let pause_binding = if self.at_directive(".pause") {
            self.pos += ".pause".len();
            self.skip_inline();
            let keys = self.parse_hotkeys()?;
            self.skip_trivia();
            Some(keys)
        } else {
            None
        };

        let pause_hook = if self.at_directive(".pause_hook") {
            self.pos += ".pause_hook".len();
            let command = self.parse_command()?;
            self.skip_trivia();
            Some(command)
        } else {
            None
        };

        let mut app_bindings = vec![];
        let mut bindings = vec![];

        while self.peek().is_some() {
            let keys = self.parse_hotkeys()?;
            self.skip_inline();

            match self.peek() {
                Some('[') => {
                    if !bindings.is_empty() {
                        return Err(ParseError::new(
                            "app bindings must come before plain bindings",
                            self.here(),
                            "this `[` starts an app binding",
                        )
                        .with_help("move this block above the first `keys : command` binding"));
                    }

                    let mapped = self.parse_app_binding(&keys)?;
                    app_bindings.push((keys, mapped));
                }
                Some(':') => {
                    self.pos += 1;
                    let command = self.parse_command()?;
                    bindings.push(HotkeyBinding {
                        keys,
                        command,
                        process_name: None,
                    });
                }
                _ => {
                    return Err(ParseError::new(
                        format!("expected `:` or `[` after hotkey, found {}", self.found()),
                        self.here(),
                        "expected `:` or `[` here",
                    ));
                }
            }

            self.skip_trivia();
        }

        if bindings.is_empty() {
            return Err(ParseError::new(
                "expected at least one hotkey binding",
                self.here(),
                "no bindings found",
            )
            .with_help("add a binding such as `alt + h : echo hello`"));
        }

        Ok(Whkdrc {
            shell,
            app_bindings,
            bindings,
            pause_binding,
            pause_hook,
        })
    }

    fn parse_shell(&mut self) -> Result<Shell, ParseError> {
        if !self.at_directive(".shell") {
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

    /// The rest of the line, up to a comment, trimmed
    fn parse_command(&mut self) -> Result<String, ParseError> {
        self.skip_inline();

        let command = self.take_while(|c| c != '\n' && c != '#').trim_end();
        if command.is_empty() {
            return Err(ParseError::new(
                format!("expected a command, found {}", self.found()),
                self.here(),
                "expected a command here",
            ));
        }

        Ok(command.to_string())
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

            let process_name = self.parse_process_name()?;
            self.skip_inline();
            self.expect(':').map_err(|error| {
                error.with_note(open..open + 1, "inside this `[` block; is a `]` missing?")
            })?;
            let command = self.parse_command()?;

            mapped.push(HotkeyBinding {
                keys: keys.to_vec(),
                command,
                process_name: Some(process_name),
            });
        }

        if mapped.is_empty() {
            return Err(ParseError::new(
                "empty app binding",
                open..self.pos,
                "expected at least one `process : command` entry",
            ));
        }

        Ok(mapped)
    }

    /// One or more identifiers separated by whitespace, joined with a single space
    fn parse_process_name(&mut self) -> Result<String, ParseError> {
        if !self.peek().is_some_and(is_ident_start) {
            return Err(ParseError::new(
                format!("expected a process name, found {}", self.found()),
                self.here(),
                "expected a process name (or `Default`) here",
            ));
        }

        let mut words = vec![];

        loop {
            words.push(self.take_while(is_ident_continue));
            self.skip_inline();

            if !self.peek().is_some_and(is_ident_start) {
                break;
            }
        }

        Ok(words.join(" "))
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
                process_name: None,
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
                process_name: None,
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
                        process_name: Option::from("Firefox".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        process_name: Option::from("Google Chrome".to_string()),
                    },
                ],
            )],
            bindings: vec![
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("h")],
                    command: String::from("komorebic focus left"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("j")],
                    command: String::from("komorebic focus down"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("k")],
                    command: String::from("komorebic focus up"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("l")],
                    command: String::from("komorebic focus right"),
                    process_name: None,
                },
                HotkeyBinding {
                    keys: vec![String::from("alt"), String::from("1")],
                    command: String::from("komorebic focus-workspace 0"),
                    process_name: None,
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
                process_name: None,
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
                        process_name: Option::from("Default".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello firefox""#),
                        process_name: Option::from("Firefox".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from(r#"echo "hello chrome""#),
                        process_name: Option::from("Google Chrome".to_string()),
                    },
                    HotkeyBinding {
                        keys: vec![String::from("alt"), String::from("n")],
                        command: String::from("Ignore"),
                        process_name: Option::from("Zen Browser".to_string()),
                    },
                ],
            )],
            bindings: vec![HotkeyBinding {
                keys: vec![String::from("alt"), String::from("h")],
                command: String::from(r#"echo "Hello""#),
                process_name: None,
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
                process_name: None,
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
                process_name: None,
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
                process_name: None,
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
        let names: Vec<_> = output.app_bindings[0]
            .1
            .iter()
            .map(|b| b.process_name.clone().unwrap())
            .collect();
        assert_eq!(names, vec!["DefaultApp", "Default"]);
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
        assert!(e.message.starts_with("expected `:` or `[`"));
    }

    #[test]
    fn test_error_unterminated_bracket() {
        let src = ".shell cmd\nalt + n [\n Firefox : a\n";
        let e = err(src);
        assert_eq!(e.message, "unterminated `[`");
        assert_eq!(&src[e.notes[0].0.clone()], "[");
    }

    #[test]
    fn test_error_app_binding_after_binding() {
        let e = err(".shell cmd\nf1 : x\nalt + n [\n Firefox : a\n]\n");
        assert_eq!(e.message, "app bindings must come before plain bindings");
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
}
