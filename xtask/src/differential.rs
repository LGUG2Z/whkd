//! A differential test of the tree-sitter grammar against the real parser.
//!
//! The grammar's structure is written by hand, so this is what keeps it honest: the fixtures and a
//! few thousand deterministic mutations of them are run through both, and they must agree on
//! whether the input is a valid whkdrc. The only disagreements allowed are the documented ones in
//! [`known_difference`], which are things a grammar cannot (or deliberately does not) express.
//!
//! It needs `nvim` (as a batch tree-sitter engine) and the tree-sitter CLI (to build the parser
//! library), so it is ignored by default: `cargo test -p xtask -- --ignored`. Set
//! `WHKD_TREE_SITTER_LIB` to a prebuilt library to skip building it.

use crate::repository_root;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next()).unwrap_or(0) % n.max(1)
    }
}

/// Pieces of syntax to splice in, so that mutations stay close to real configs
const SNIPPETS: &[&str] = &[
    " ",
    "\t",
    "\n",
    "\r\n",
    "#",
    ":",
    "[",
    "]",
    "+",
    ".",
    "a",
    "1",
    "_",
    "-",
    "=",
    "!=",
    "&",
    "|",
    "when",
    " when ",
    "app = ",
    "layout = bsp",
    "Default",
    "Ignore",
    ".shell cmd\n",
    ".pause alt + p\n",
    ".pause_hook x\n",
    ".ignore Code\n",
    ".when x : y\n",
    ".when ",
    "!",
    "!x",
    " x &",
    ".ignore ",
    " \\",
    " \\\n",
    "{",
    "}",
    ",",
    "{a,b}",
    "{_,shift +}",
    "{1-9}",
    "{a,b} ",
    "Google Chrome",
    "UltrawideVerticalStack",
    "scrolling",
];

fn mutate(rng: &mut Lcg, seed: &str) -> String {
    let mut lines: Vec<String> = seed.split('\n').map(String::from).collect();

    for _ in 0..=rng.below(3) {
        let line = rng.below(lines.len());
        match rng.below(7) {
            0 if lines.len() > 1 => {
                lines.remove(line);
            }
            1 => {
                let copy = lines[line].clone();
                lines.insert(line, copy);
            }
            2 if lines.len() > 1 => {
                let at = rng.below(lines.len() - 1);
                lines.swap(at, at + 1);
            }
            3 => {
                let mut chars: Vec<char> = lines[line].chars().collect();
                if !chars.is_empty() {
                    chars.remove(rng.below(chars.len()));
                }
                lines[line] = chars.into_iter().collect();
            }
            4 => {
                let mut chars: Vec<char> = lines[line].chars().collect();
                let at = rng.below(chars.len() + 1);
                for (offset, c) in SNIPPETS[rng.below(SNIPPETS.len())].chars().enumerate() {
                    chars.insert(at + offset, c);
                }
                lines[line] = chars.into_iter().collect();
            }
            5 => lines[line].push_str(SNIPPETS[rng.below(SNIPPETS.len())]),
            _ => lines[line] = format!("{}{}", SNIPPETS[rng.below(SNIPPETS.len())], lines[line]),
        }
    }

    lines.join("\n")
}

fn parser_accepts(source: &str) -> bool {
    whkd_parser::parse_full(source, None).is_ok()
}

fn run(command: &mut Command) -> String {
    let output = command.output().expect("could not start the command");
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn library(grammar_dir: &Path, scratch: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("WHKD_TREE_SITTER_LIB") {
        return PathBuf::from(path);
    }

    let library = scratch.join("whkdrc.dll");
    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    run(Command::new(npx)
        .args(["tree-sitter", "build", "-o"])
        .arg(&library)
        .current_dir(grammar_dir));
    library
}

/// Whether tree-sitter reports a syntax error, for every source, using Neovim as the engine
fn tree_sitter_errors(sources: &[String]) -> Vec<bool> {
    let grammar_dir = repository_root().join("editors/tree-sitter-whkdrc");
    let scratch = std::env::temp_dir().join("whkd-differential");
    std::fs::create_dir_all(&scratch).unwrap();

    let library = library(&grammar_dir, &scratch);
    let input = scratch.join("sources.json");
    let output = scratch.join("results.json");
    std::fs::write(&input, serde_json::to_string(sources).unwrap()).unwrap();
    let _ = std::fs::remove_file(&output);

    run(Command::new("nvim")
        .args(["--headless", "-l"])
        .arg(grammar_dir.join("test/differential.lua"))
        .arg(&library)
        .arg(&input)
        .arg(&output));

    serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap()
}

/// Disagreements that are expected, and why. Everything else is a bug in the grammar or the parser.
///
/// `parser_accepts` is what the parser said, `tree_sitter_accepts` what the grammar said.
fn known_difference(
    source: &str,
    parser_accepts: bool,
    tree_sitter_accepts: bool,
) -> Option<&'static str> {
    // the parser does not insist on a newline after a directive, so `.shell cmd.pause alt + p`
    // is accepted, while the grammar (like a person) reads one directive per line
    if parser_accepts && !tree_sitter_accepts {
        let directives_run_together = source.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with(".shell") && line.len() > 1 && line[1..].contains('.')
                || line.starts_with(".pause")
                    && !line.starts_with(".pause_hook")
                    && line[1..].contains('.')
        });

        if directives_run_together {
            return Some("directives on one line");
        }
    }

    // the parser does not insist on a line feed after a directive or after the `]` that closes a
    // block, so something can follow on the same line, which the grammar (like a person) does not read
    if parser_accepts && !tree_sitter_accepts {
        let more_on_the_line = source.lines().any(|line| {
            let code = line.split('#').next().unwrap_or("").trim();
            let after_shell = code.starts_with(".shell") && code.split_whitespace().count() > 2;
            let after_block = code.starts_with(']') && code.len() > 1;
            // an entry can be on the line that opens the block
            let after_open = code
                .split_once('[')
                .is_some_and(|(_, after)| !after.trim().is_empty());
            after_shell || after_block || after_open
        });

        if more_on_the_line {
            return Some("something after a directive or a block on the same line");
        }
    }

    // a carriage return on its own, such as at the very end of a file, is white space to the parser,
    // while the grammar's newline is a line feed with an optional carriage return before it
    if parser_accepts && !tree_sitter_accepts {
        let lone = source
            .char_indices()
            .any(|(at, c)| c == '\r' && !source[at + 1..].starts_with('\n'));
        if lone {
            return Some("a lone carriage return");
        }
    }

    // a key named `when` can be written right after a group, where the grammar has a guard
    if parser_accepts && !tree_sitter_accepts && source.contains("}when") {
        return Some("a key named when next to a group");
    }

    // what a condition may be called, and that it is defined once, is for the parser to say. To the
    // grammar a name is a name
    if !parser_accepts && tree_sitter_accepts && source.contains(".when") {
        return Some("the name of a condition");
    }

    // and so is which names can be compared with a value: to the grammar, any can
    if !parser_accepts && tree_sitter_accepts {
        let compared = source.match_indices('=').any(|(at, _)| {
            let before = source[..at].trim_end_matches('!').trim_end();

            // the condition that is being compared: what is after the last `when`, `&`, `|`, `:` or
            // `[` or line break, which to the grammar can be any run of words
            let start = before
                .rfind(['&', '|', ':', '[', '\n'])
                .map_or(0, |i| i + 1);
            let term = before[start..].trim();
            let term = term.split_once(" when ").map_or(term, |(_, after)| after);
            let words: Vec<&str> = term.split_whitespace().collect();

            !words.is_empty() && !(words.len() == 1 && (words[0] == "app" || words[0] == "layout"))
        });

        if compared {
            return Some("a name compared with a value");
        }
    }

    // `Default` is something to the parser, and cannot be one of several conditions. To the grammar
    // it is a name like any other when it is not on its own
    if !parser_accepts && tree_sitter_accepts {
        let squashed: String = source.chars().filter(|c| !c.is_whitespace()).collect();

        if squashed.contains("&Default") || squashed.contains("Default&") {
            return Some("`Default` with other conditions");
        }
    }

    // which names a `!` can be put against is for the parser to say, since it is the conditions that
    // have been defined. To the grammar they are names
    if !parser_accepts && tree_sitter_accepts {
        let negation = source
            .match_indices('!')
            .any(|(at, _)| !source[at + 1..].starts_with('='));

        if negation {
            return Some("the opposite of a condition that is not defined");
        }
    }

    // what a group stands for is for the parser to say: whether the keys that come out of it are
    // keys, and whether the groups of the command are as long as those of the hotkey. To the
    // grammar a group is something between braces that can be next to a key
    if !parser_accepts && tree_sitter_accepts {
        let groups = source.lines().any(|line| {
            line.split([':', '[', '#'])
                .next()
                .is_some_and(|hotkey| hotkey.contains('{'))
        });

        if groups {
            return Some("what a group stands for");
        }
    }

    // the largest cooldown is for the parser to say: to the grammar it is any number
    if !parser_accepts
        && tree_sitter_accepts
        && (source.contains(".wheel_cooldown") || source.contains(".gesture_cooldown"))
    {
        return Some("a cooldown that is too long");
    }

    // where a mouse input may go, and with which keys, is for the parser to say: to the grammar they
    // are keys like any other
    if !parser_accepts && tree_sitter_accepts {
        let lower = source.to_lowercase();
        if whkd_core::MouseInput::ALL
            .iter()
            .any(|input| lower.contains(input.name()))
            || whkd_core::GestureInput::ALL
                .iter()
                .any(|input| lower.contains(input.name()))
        {
            return Some("a mouse input or a gesture in the wrong place");
        }
    }

    // a grammar cannot look ahead for a word boundary, so it reads `.shellpwsh` as `.shell pwsh`
    // (and `whenFirefox` as `when Firefox`), which the parser rejects
    if !parser_accepts && tree_sitter_accepts {
        let no_space_after_directive = [
            ".shell",
            ".pause_hook",
            ".pause",
            ".wheel_cooldown",
            ".gesture_cooldown",
            "when",
        ]
        .iter()
        .any(|directive| {
            source.match_indices(directive).any(|(at, _)| {
                source[at + directive.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        });

        if no_space_after_directive {
            return Some("no space after a keyword");
        }
    }

    None
}

#[test]
#[ignore = "needs nvim and the tree-sitter CLI"]
fn tree_sitter_agrees_with_the_parser() {
    let mut seeds: Vec<String> = ["editors/fixtures", "editors/fixtures/invalid"]
        .iter()
        .flat_map(|dir| crate::tests::fixtures(dir))
        .map(|(_, source)| source.replace("\r\n", "\n"))
        .collect();
    seeds.sort();

    let mut rng = Lcg(0x5eed);
    let mut sources = seeds.clone();
    let iterations: usize = std::env::var("WHKD_DIFFERENTIAL_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4000);

    for _ in 0..iterations {
        let seed = &seeds[rng.below(seeds.len())];
        sources.push(mutate(&mut rng, seed));
    }

    let errors = tree_sitter_errors(&sources);
    assert_eq!(errors.len(), sources.len());

    let mut agreed = 0;
    let mut explained = 0;
    let mut unexplained: Vec<(&String, bool, bool)> = vec![];

    for (source, has_error) in sources.iter().zip(&errors) {
        let (parser, tree_sitter) = (parser_accepts(source), !has_error);
        if parser == tree_sitter {
            agreed += 1;
        } else if known_difference(source, parser, tree_sitter).is_some() {
            explained += 1;
        } else {
            unexplained.push((source, parser, tree_sitter));
        }
    }

    println!(
        "{} inputs: {agreed} agree, {explained} known differences, {} unexplained",
        sources.len(),
        unexplained.len()
    );

    unexplained.sort_by_key(|(source, ..)| source.len());
    unexplained.dedup_by_key(|(source, ..)| (*source).clone());
    for (source, parser, tree_sitter) in unexplained.iter().take(25) {
        println!("--- parser accepts: {parser}, tree-sitter accepts: {tree_sitter}\n{source:?}");
    }

    assert!(
        unexplained.is_empty(),
        "{} inputs where the grammar and the parser disagree",
        unexplained.len()
    );
}
