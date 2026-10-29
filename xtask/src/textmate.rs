//! The TextMate grammar (VS Code, Sublime Text, JetBrains, GitHub, `bat`, Shiki).
//!
//! The structure is written here; every word it matches (directives, shells, keywords, layouts,
//! modifier keys) comes from the [`Vocabulary`], which is read from the parser. Commands are
//! embedded as PowerShell or batch, following the `.shell` directive: a rule that opens at
//! `.shell pwsh` and never closes covers the rest of the file, so everything below it is scoped
//! for that shell.

use crate::vocabulary::Vocabulary;
use serde_json::json;
use serde_json::Map;
use serde_json::Value;
use whkd_core::Shell;
use whkd_parser::syntax;

const SCOPE: &str = "source.whkdrc";

const IDENT: &str = r"[\p{L}_][\p{L}\p{N}_]*";
const BEFORE_WORD: &str = r"(?<![\p{L}\p{N}_])";
const AFTER_WORD: &str = r"(?![\p{L}\p{N}_])";

/// A rule that can never match, so a `begin` that uses it as `end` lasts to the end of the file
const NEVER: &str = r"\b\B";

/// How the command part of a binding is scoped
struct Variant {
    /// Suffix for the rule names that depend on the shell
    suffix: &'static str,
    /// The grammar to embed, and the scope of the embedded region (VS Code maps the scope to a
    /// language through `embeddedLanguages`)
    embedded: Option<(&'static str, &'static str)>,
}

const POWERSHELL: Variant = Variant {
    suffix: "powershell",
    embedded: Some(("source.powershell", "meta.embedded.line.powershell")),
};

const BATCH: Variant = Variant {
    suffix: "batch",
    embedded: Some(("source.batchfile", "meta.embedded.line.batchfile")),
};

const PLAIN: Variant = Variant {
    suffix: "plain",
    embedded: None,
};

impl Variant {
    fn content_name(&self) -> &'static str {
        self.embedded
            .map_or("string.unquoted.command.whkdrc", |(_, scope)| scope)
    }

    /// What goes inside a command: `Ignore` on its own, then the embedded shell
    fn command_patterns(&self) -> Vec<Value> {
        let mut patterns = vec![json!({
            "match": format!(
                r"\G[ \t]*({})[ \t]*(?=#|$)",
                syntax::IGNORE
            ),
            "captures": { "1": { "name": "keyword.other.ignore.whkdrc" } }
        })];

        if let Some((grammar, _)) = self.embedded {
            patterns.push(json!({ "include": grammar }));
        }

        patterns
    }
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn alternation(words: &[String]) -> String {
    let mut words: Vec<&String> = words.iter().collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    words
        .iter()
        .map(|word| escape(word))
        .collect::<Vec<_>>()
        .join("|")
}

pub fn grammar(vocabulary: &Vocabulary) -> Value {
    let key = format!("(?:{IDENT}|[0-9]+)");
    let keys = format!(r"{key}(?:[ \t]*\+[ \t]*{key})*");
    let layouts = vocabulary
        .layouts
        .iter()
        .map(|layout| layout.pattern.as_str())
        .collect::<Vec<_>>()
        .join("|");

    let directive = |name: &str| escape(name);
    let [shell, pause, pause_hook] = [syntax::SHELL, syntax::PAUSE, syntax::PAUSE_HOOK];

    let mut repository = Map::new();
    let mut add = |name: String, rule: Value| {
        repository.insert(name, rule);
    };

    add(
        "comment".into(),
        json!({
            "name": "comment.line.number-sign.whkdrc",
            "match": format!("{}.*$", escape(&syntax::COMMENT.to_string()))
        }),
    );

    add(
        "hotkey".into(),
        json!({
            "patterns": [
                {
                    "name": "keyword.other.modifier.whkdrc",
                    "match": format!(
                        "(?i:{BEFORE_WORD}(?:{}){AFTER_WORD})",
                        alternation(&vocabulary.modifiers)
                    )
                },
                {
                    "name": "keyword.operator.key-separator.whkdrc",
                    "match": escape(&syntax::KEY_SEPARATOR.to_string())
                },
                {
                    "name": "constant.character.key.mouse.whkdrc",
                    "match": format!(
                        "(?i:{BEFORE_WORD}(?:{}){AFTER_WORD})",
                        alternation(&vocabulary.mouse_inputs)
                    )
                },
                { "name": "constant.character.key.whkdrc", "match": key }
            ]
        }),
    );

    // conditions are the same everywhere: in a `when` guard and on the lines of a block
    let [equals, not_equals, or, and] = [
        escape(&syntax::EQUALS.to_string()),
        escape(syntax::NOT_EQUALS),
        escape(&syntax::OR.to_string()),
        escape(&syntax::AND.to_string()),
    ];
    let comparison = format!("({not_equals}|{equals})");
    let value_end = format!(
        "(?=[{}{}{}]|$)",
        escape(&syntax::DELIMITER.to_string()),
        escape(&syntax::AND.to_string()),
        escape(&syntax::COMMENT.to_string())
    );
    let process = r"[\p{L}\p{N}_.-]+(?:[ \t]+[\p{L}\p{N}_.-]+)*";

    add(
        "layout-value".into(),
        json!({
            "name": "constant.other.layout.whkdrc",
            "match": format!(r"(?<![\p{{L}}\p{{N}}_-])(?:{layouts})(?![\p{{L}}\p{{N}}_-])")
        }),
    );

    add(
        "condition".into(),
        json!({
            "patterns": [
                {
                    "begin": format!(
                        "{BEFORE_WORD}({}){AFTER_WORD}[ \\t]*{comparison}",
                        syntax::LAYOUT
                    ),
                    "beginCaptures": {
                        "1": { "name": "variable.parameter.condition.whkdrc" },
                        "2": { "name": "keyword.operator.comparison.whkdrc" }
                    },
                    "end": value_end,
                    "patterns": [
                        { "include": "#layout-value" },
                        { "name": "keyword.operator.logical.whkdrc", "match": or },
                        {
                            "name": "invalid.illegal.unknown-layout.whkdrc",
                            "match": r"[\p{L}\p{N}_-]+"
                        }
                    ]
                },
                {
                    "begin": format!(
                        "{BEFORE_WORD}({}){AFTER_WORD}[ \\t]*{comparison}",
                        syntax::APP
                    ),
                    "beginCaptures": {
                        "1": { "name": "variable.parameter.condition.whkdrc" },
                        "2": { "name": "keyword.operator.comparison.whkdrc" }
                    },
                    "end": value_end,
                    "patterns": [
                        { "name": "keyword.operator.logical.whkdrc", "match": or },
                        { "name": "entity.name.type.process.whkdrc", "match": process }
                    ]
                },
                {
                    "name": "invalid.illegal.unknown-condition.whkdrc",
                    "match": format!("{BEFORE_WORD}{IDENT}(?=[ \\t]*(?:{not_equals}|{equals}))")
                },
                { "name": "keyword.operator.comparison.whkdrc", "match": format!("{not_equals}|{equals}") },
                { "name": "keyword.operator.logical.whkdrc", "match": and },
                {
                    "name": "keyword.other.default.whkdrc",
                    "match": format!(
                        "{BEFORE_WORD}{}{AFTER_WORD}(?=[ \\t]*(?::|#|$))",
                        syntax::DEFAULT
                    )
                },
                {
                    "name": "entity.name.type.process.whkdrc",
                    "match": format!(r"{IDENT}(?:[ \t]+{IDENT})*")
                }
            ]
        }),
    );

    // the shell-dependent rules, once per way of scoping a command
    for variant in [&POWERSHELL, &BATCH, &PLAIN] {
        let suffix = variant.suffix;
        let command = format!("command-{suffix}");

        add(
            command.clone(),
            json!({
                "begin": escape(&syntax::DELIMITER.to_string()),
                "beginCaptures": { "0": { "name": "punctuation.separator.delimiter.whkdrc" } },
                "end": "(?=#)|$",
                "contentName": variant.content_name(),
                "patterns": variant.command_patterns()
            }),
        );

        add(
            format!("binding-{suffix}"),
            json!({
                "begin": format!(r"^[ \t]*({keys})[ \t]*({})", escape(&syntax::DELIMITER.to_string())),
                "beginCaptures": {
                    "1": { "patterns": [{ "include": "#hotkey" }] },
                    "2": { "name": "punctuation.separator.delimiter.whkdrc" }
                },
                "end": "(?=#)|$",
                "contentName": variant.content_name(),
                "patterns": variant.command_patterns()
            }),
        );

        add(
            format!("when-{suffix}"),
            json!({
                "begin": format!(r"^[ \t]*({keys})[ \t]+({}){AFTER_WORD}", syntax::WHEN),
                "beginCaptures": {
                    "1": { "patterns": [{ "include": "#hotkey" }] },
                    "2": { "name": "keyword.control.when.whkdrc" }
                },
                "end": "(?=#)|$",
                "patterns": [
                    { "include": "#condition" },
                    { "include": format!("#{command}") }
                ]
            }),
        );

        add(
            format!("entry-{suffix}"),
            json!({
                "begin": r"^(?=[ \t]*[\p{L}_])",
                "end": "(?=#)|$",
                "patterns": [
                    { "include": "#condition" },
                    { "include": format!("#{command}") }
                ]
            }),
        );

        add(
            format!("block-{suffix}"),
            json!({
                "begin": format!(
                    r"^[ \t]*({keys})[ \t]*({})",
                    escape(&syntax::BLOCK_OPEN.to_string())
                ),
                "beginCaptures": {
                    "1": { "patterns": [{ "include": "#hotkey" }] },
                    "2": { "name": "punctuation.definition.block.begin.whkdrc" }
                },
                "end": format!(r"^[ \t]*({})", escape(&syntax::BLOCK_CLOSE.to_string())),
                "endCaptures": { "1": { "name": "punctuation.definition.block.end.whkdrc" } },
                "patterns": [
                    { "include": "#comment" },
                    { "include": format!("#entry-{suffix}") }
                ]
            }),
        );

        add(
            format!("pause-hook-{suffix}"),
            json!({
                "begin": format!(r"^[ \t]*({}){AFTER_WORD}", directive(pause_hook)),
                "beginCaptures": { "1": { "name": "keyword.control.directive.whkdrc" } },
                "end": "(?=#)|$",
                "contentName": variant.content_name(),
                "patterns": variant.command_patterns()
            }),
        );
    }

    add(
        "pause".into(),
        json!({
            "match": format!(r"^[ \t]*({}){AFTER_WORD}[ \t]*({keys})?", directive(pause)),
            "captures": {
                "1": { "name": "keyword.control.directive.whkdrc" },
                "2": { "patterns": [{ "include": "#hotkey" }] }
            }
        }),
    );

    add(
        "wheel-cooldown".into(),
        json!({
            "match": format!(
                r"^[ \t]*({}){AFTER_WORD}[ \t]*([0-9]+)?",
                directive(syntax::WHEEL_COOLDOWN)
            ),
            "captures": {
                "1": { "name": "keyword.control.directive.whkdrc" },
                "2": { "name": "constant.numeric.whkdrc" }
            }
        }),
    );

    // `.shell` with something other than a shell after it
    add(
        "shell-unknown".into(),
        json!({
            "match": format!(r"^[ \t]*({}){AFTER_WORD}(?:[ \t]+([^\s#]+))?", directive(shell)),
            "captures": {
                "1": { "name": "keyword.control.directive.whkdrc" },
                "2": { "name": "invalid.illegal.unknown-shell.whkdrc" }
            }
        }),
    );

    // everything that can follow the `.shell` line, for a given way of scoping commands
    let body = |variant: &Variant| -> Vec<Value> {
        let suffix = variant.suffix;
        vec![
            json!({ "include": "#comment" }),
            json!({ "include": format!("#pause-hook-{suffix}") }),
            json!({ "include": "#pause" }),
            json!({ "include": "#wheel-cooldown" }),
            json!({ "include": format!("#block-{suffix}") }),
            json!({ "include": format!("#when-{suffix}") }),
            json!({ "include": format!("#binding-{suffix}") }),
        ]
    };

    // one rule per family of shells: it opens at `.shell <name>` and never closes
    for (name, variant) in [("powershell", &POWERSHELL), ("batch", &BATCH)] {
        let shells: Vec<String> = Shell::ALL
            .into_iter()
            .filter(|shell| {
                // an exhaustive match, so that a new shell has to be assigned to a family here
                let family = match shell {
                    Shell::Pwsh | Shell::Powershell => "powershell",
                    Shell::Cmd => "batch",
                };
                family == name
            })
            .map(|shell| shell.name().to_string())
            .collect();

        add(
            format!("shell-{name}"),
            json!({
                "begin": format!(
                    r"^[ \t]*({})[ \t]+({}){AFTER_WORD}",
                    directive(shell),
                    alternation(&shells)
                ),
                "beginCaptures": {
                    "1": { "name": "keyword.control.directive.whkdrc" },
                    "2": { "name": "constant.language.shell.whkdrc" }
                },
                "end": NEVER,
                "name": format!("meta.shell.{name}.whkdrc"),
                "patterns": body(variant)
            }),
        );
    }

    let mut top_level = vec![
        json!({ "include": "#shell-powershell" }),
        json!({ "include": "#shell-batch" }),
        json!({ "include": "#shell-unknown" }),
    ];
    top_level.extend(body(&PLAIN));

    json!({
        "comment": crate::GENERATED_NOTICE,
        "$schema": "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json",
        "name": "whkdrc",
        "scopeName": SCOPE,
        "fileTypes": ["whkdrc"],
        "patterns": top_level,
        "repository": repository
    })
}
