#![warn(clippy::all)]
#![allow(clippy::missing_errors_doc, clippy::redundant_pub_crate)]

use clap::Parser;
use clap::Subcommand;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use lazy_static::lazy_static;
use mouse::Modifiers;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use whkd_core::select;
use whkd_core::Condition;
use whkd_core::GestureInput;
use whkd_core::HotkeyBinding;
use whkd_core::MouseInput;
use whkd_core::Shell;
use whkd_core::State;
use whkd_core::Whkdrc;
use whkd_core::DEFAULT_GESTURE_COOLDOWN_MS;
use whkd_core::DEFAULT_WHEEL_COOLDOWN_MS;
use win_hotkeys::error::WHKError;
use win_hotkeys::HotkeyManager;
use win_hotkeys::VKey;

mod komorebi;
mod mouse;
mod touchpad;

lazy_static! {
    static ref WHKDRC: Whkdrc = {
        let home = default_config_path().unwrap_or_else(|error| panic!("{error}"));
        whkd_parser::load_with(&home, &key_validator()).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1)
        })
    };
    static ref SESSION_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);
}

/// The config file defaults to `~/.config/whkdrc`, or `<WHKD_CONFIG_HOME>/whkdrc`
fn default_config_path() -> Result<PathBuf> {
    let mut home = match std::env::var("WHKD_CONFIG_HOME") {
        Ok(home_path) => {
            let home = PathBuf::from(&home_path);

            if !home.as_path().is_dir() {
                return Err(eyre!(
                    "$Env:WHKD_CONFIG_HOME is set to '{home_path}', which is not a valid directory",
                ));
            }

            home
        }
        Err(_) => dirs::home_dir()
            .ok_or_else(|| eyre!("no home directory found"))?
            .join(".config"),
    };

    home.push("whkdrc");
    Ok(home)
}

/// The explicit path if there is one, otherwise the default, exiting if there is no such file
fn existing_config_path(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path,
        None => default_config_path()?,
    };

    if !path.is_file() {
        eprintln!("{}: no such file", path.display());
        std::process::exit(1);
    }

    Ok(path)
}

/// Format a whkdrc, refusing to touch one that does not parse
fn fmt(path: Option<PathBuf>, write: bool, check: bool) -> Result<()> {
    let path = existing_config_path(path)?;
    let keys = key_validator();

    let src = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        eprintln!("{}: {error}", path.display());
        std::process::exit(1)
    });

    let original = match whkd_parser::parse_full(&src, Some(&keys)) {
        Ok(parsed) => parsed.whkdrc,
        Err(error) => {
            eprintln!(
                "{}",
                whkd_parser::WhkdError::Parse {
                    path,
                    src,
                    error: Box::new(error),
                }
            );
            std::process::exit(1)
        }
    };

    let formatted = whkd_parser::format::format(&src);

    // the formatter must never change what a whkdrc means
    let same = whkd_parser::parse_full(&formatted, Some(&keys))
        .is_ok_and(|parsed| parsed.whkdrc == original);
    if !same {
        eprintln!(
            "{}: the formatter produced a different configuration, refusing to use its output (this is a bug in `whkd fmt`)",
            path.display()
        );
        std::process::exit(1);
    }

    if check {
        if formatted != src {
            eprintln!("{}: not formatted", path.display());
            std::process::exit(1);
        }
    } else if write {
        if formatted == src {
            println!("{}: unchanged", path.display());
        } else {
            std::fs::write(&path, formatted)?;
            println!("{}: formatted", path.display());
        }
    } else {
        print!("{formatted}");
    }

    Ok(())
}

/// Check that a whkdrc parses and only uses valid key names, printing a report if it does not
fn validate(path: Option<PathBuf>) -> Result<()> {
    let path = existing_config_path(path)?;

    match whkd_parser::load_checked(&path, &key_validator()) {
        Ok(loaded) => {
            for warning in loaded.render_warnings() {
                eprintln!("{warning}");
            }

            let whkdrc = &loaded.whkdrc;

            for note in touchpad::settings::notes(whkdrc) {
                eprintln!("{note}");
            }

            let app_bindings: usize = whkdrc.app_bindings.iter().map(|(_, v)| v.len()).sum();
            println!(
                "{}: ok ({} bindings, {app_bindings} conditional bindings, shell: {}, {} warnings)",
                path.display(),
                whkdrc.bindings.len(),
                whkdrc.shell,
                loaded
                    .warnings
                    .iter()
                    .filter(|w| w.severity != whkd_parser::Severity::Advice)
                    .count(),
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1)
        }
    }
}

/// Key names whkd accepts, used to flag typos in the whkdrc
fn key_validator() -> whkd_parser::KeyValidator {
    whkd_keys::validator()
}

/// What fires a binding: a keyboard hotkey, or a mouse input or a touchpad gesture held with
/// keyboard modifiers
#[derive(Debug, Clone)]
pub enum Trigger {
    Keyboard {
        mod_keys: Vec<VKey>,
        vkey: VKey,
    },
    Mouse {
        mod_keys: Vec<VKey>,
        input: MouseInput,
    },
    Gesture {
        mod_keys: Vec<VKey>,
        input: GestureInput,
    },
}

/// Identifies a hotkey however it was written in the whkdrc
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TriggerId {
    Keyboard(Vec<u16>, u16),
    Mouse(Vec<u16>, MouseInput),
    Gesture(Vec<u16>, GestureInput),
}

#[derive(Debug, Clone)]
pub struct HkmData {
    pub trigger: Trigger,
    pub command: String,
    pub condition: Condition,
}

fn vk_codes(mod_keys: &[VKey]) -> Vec<u16> {
    let mut codes: Vec<u16> = mod_keys.iter().map(|key| key.to_vk_code()).collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

impl HkmData {
    fn id(&self) -> TriggerId {
        match &self.trigger {
            Trigger::Keyboard { mod_keys, vkey } => {
                TriggerId::Keyboard(vk_codes(mod_keys), vkey.to_vk_code())
            }
            Trigger::Mouse { mod_keys, input } => TriggerId::Mouse(vk_codes(mod_keys), *input),
            Trigger::Gesture { mod_keys, input } => TriggerId::Gesture(vk_codes(mod_keys), *input),
        }
    }

    /// What a gesture binding is found by, however it was written in the whkdrc
    fn gesture_key(&self) -> Option<(GestureInput, Modifiers)> {
        match &self.trigger {
            Trigger::Gesture { mod_keys, input } => Some((*input, Modifiers::from_keys(mod_keys))),
            Trigger::Keyboard { .. } | Trigger::Mouse { .. } => None,
        }
    }

    /// What a mouse binding is found by, however it was written in the whkdrc
    fn mouse_key(&self) -> Option<(MouseInput, Modifiers)> {
        match &self.trigger {
            Trigger::Mouse { mod_keys, input } => Some((*input, Modifiers::from_keys(mod_keys))),
            Trigger::Keyboard { .. } | Trigger::Gesture { .. } => None,
        }
    }

    pub fn register(&self, hkm: &mut HotkeyManager<()>, shell: Shell) -> Result<()> {
        // mouse bindings are not registered with the hotkey manager: see mouse::start
        let Trigger::Keyboard { mod_keys, vkey } = &self.trigger else {
            return Ok(());
        };

        let cmd = self.command.clone();

        if let Err(error) = hkm.register_hotkey(*vkey, mod_keys.as_slice(), move || {
            run_command(shell, &cmd);
        }) {
            eprintln!(
                "Unable to bind '{mod_keys:?} + {vkey}' to '{}' (error: {error}), ignoring this binding and continuing...",
                self.command
            );
        }

        Ok(())
    }
}

impl TryFrom<&HotkeyBinding> for HkmData {
    type Error = WHKError;

    fn try_from(value: &HotkeyBinding) -> Result<Self, Self::Error> {
        let (last, mods) = value
            .keys
            .split_last()
            .expect("a binding has at least one key");

        let mod_keys = mods
            .iter()
            .map(|key| VKey::from_keyname(key))
            .collect::<Result<Vec<_>, _>>()?;

        // only modifiers can be held down with a mouse input or a gesture, since that is all that
        // can be read when one happens
        let only_modifiers = |what: &str| -> Result<(), WHKError> {
            match mods
                .iter()
                .zip(&mod_keys)
                .find(|(_, key)| Modifiers::of(**key).is_none())
            {
                Some((name, _)) => Err(WHKError::InvalidKey(format!(
                    "{name} (only alt, ctrl, shift and win can be held with {what})"
                ))),
                None => Ok(()),
            }
        };

        // the parser guarantees that a mouse input or a gesture is the last key
        let trigger = if let Some(input) = MouseInput::parse(last) {
            only_modifiers("a mouse input")?;
            Trigger::Mouse { mod_keys, input }
        } else if let Some(input) = GestureInput::parse(last) {
            only_modifiers("a gesture")?;
            Trigger::Gesture { mod_keys, input }
        } else {
            Trigger::Keyboard {
                mod_keys,
                vkey: VKey::from_keyname(last)?,
            }
        };

        Ok(Self {
            trigger,
            command: value.command.clone(),
            condition: value.condition.clone(),
        })
    }
}

#[derive(Parser)]
#[clap(author, about, version)]
struct Cli {
    /// Path to whkdrc
    #[clap(action, short, long)]
    config: Option<PathBuf>,
    #[clap(subcommand)]
    command: Option<SubCommand>,
}

#[derive(Subcommand)]
enum SubCommand {
    /// Check a whkdrc for syntax errors and invalid key names without starting the daemon
    Validate {
        /// Path to whkdrc (defaults to the usual lookup: $WHKD_CONFIG_HOME or ~/.config)
        path: Option<PathBuf>,
    },
    /// Format a whkdrc, keeping its comments, and print the result (or rewrite the file)
    Fmt {
        /// Path to whkdrc (defaults to the usual lookup: $WHKD_CONFIG_HOME or ~/.config)
        path: Option<PathBuf>,
        /// Rewrite the file in place instead of printing the result
        #[clap(short, long, conflicts_with = "check")]
        write: bool,
        /// Print nothing, and exit with a non-zero status if the file is not formatted
        #[clap(long)]
        check: bool,
    },
    /// Show what the touchpad's fingers do, as a command bound to swipe3 or swipe4 would be told
    Touchpad,
}

/// Say which precision touchpads there are, and then what the fingers on them do, in the lines a
/// stream binding's command is given. The lines go to the standard output, and the rest to the
/// standard error, so that redirecting the output records a gesture that can be replayed
fn touchpad_diagnostic() -> Result<()> {
    let touchpads = touchpad::device::find_touchpads();
    eprintln!("{} precision touchpad(s) found", touchpads.len());
    for touchpad in &touchpads {
        eprintln!(
            "{}
  {}",
            touchpad.name,
            touchpad::device::describe(touchpad)
        );
    }

    if touchpads.is_empty() {
        eprintln!("Waiting for one to be connected.");
    }

    eprintln!("Put three or four fingers on the touchpad. Press Ctrl+C to stop.");
    eprintln!("Lines are written as a command bound to swipe3 or swipe4 would be given them,");
    eprintln!("and `swipe ...` lines are what a swipe with a direction would run once.");

    touchpad::device::start_with(
        |output, _modifiers, pad| match touchpad::protocol::encode(&output, pad) {
            Some(line) => println!("{line}"),
            None => {
                if let touchpad::recogniser::Output::Swipe(gesture) = output {
                    println!("swipe {gesture}");
                }
            }
        },
        win_hotkeys::HotkeyManager::<()>::new().pause_handle(),
        touchpad::recogniser::Thresholds::default(),
    )?;

    loop {
        std::thread::park();
    }
}

/// The command to run for the entries bound to a key, given the current state
fn select_command(entries: &[HkmData]) -> Option<String> {
    let app_name = active_win_pos_rs::get_active_window()
        .unwrap_or_default()
        .app_name;

    // only bother komorebi if a condition actually asks about it
    let layout = if entries.iter().any(|e| e.condition.needs_layout()) {
        komorebi::focused_workspace_layout()
    } else {
        None
    };

    let state = State {
        app: &app_name,
        layout,
    };

    select(entries.iter().map(|e| (&e.condition, &e.command)), &state)
        .filter(|cmd| cmd.as_str() != whkd_parser::syntax::IGNORE)
        .cloned()
}

/// Run a command in the shell session, starting a new session if the old one has gone away
fn run_command(shell: Shell, cmd: &str) {
    let mut retry_with_new_session = false;

    if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
        if matches!(shell, Shell::Pwsh | Shell::Powershell) {
            println!("{cmd}");
        }

        if writeln!(session_stdin, "{cmd}").is_err() {
            retry_with_new_session = true;
        }
    }

    if retry_with_new_session && spawn_shell(shell).is_ok() {
        if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
            if matches!(shell, Shell::Pwsh | Shell::Powershell) {
                println!("{cmd}");
            }

            if writeln!(session_stdin, "{cmd}").is_err() {
                eprintln!("Unable to write to stdin session");
            }
        }
    }
}

fn spawn_shell(shell: Shell) -> Result<()> {
    let shell_binary = shell.to_string();

    match shell {
        Shell::Powershell | Shell::Pwsh => {
            let mut process = Command::new(&shell_binary)
                .stdin(Stdio::piped())
                .args(["-Command", "-"])
                .spawn()?;

            let mut stdin = process
                .stdin
                .take()
                .ok_or_else(|| eyre!("could not take stdin from powershell session"))?;

            writeln!(stdin, "$wshell = New-Object -ComObject wscript.shell")?;

            let mut session_stdin = SESSION_STDIN.lock();
            *session_stdin = Option::from(stdin);
        }
        Shell::Cmd => {
            let mut process = Command::new(&shell_binary)
                .stdin(Stdio::piped())
                .args(["-"])
                .spawn()?;

            let mut stdin = process
                .stdin
                .take()
                .ok_or_else(|| eyre!("could not take stdin from cmd session"))?;

            writeln!(stdin, "prompt $S")?;

            let mut session_stdin = SESSION_STDIN.lock();
            *session_stdin = Option::from(stdin);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    if let Some(SubCommand::Validate { path }) = cli.command {
        return validate(path.or(cli.config));
    }

    if let Some(SubCommand::Fmt { path, write, check }) = cli.command {
        return fmt(path.or(cli.config), write, check);
    }

    if let Some(SubCommand::Touchpad) = cli.command {
        return touchpad_diagnostic();
    }

    let whkdrc = cli.config.map_or_else(
        || WHKDRC.clone(),
        |config| {
            whkd_parser::load_with(&config, &key_validator()).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(1)
            })
        },
    );

    spawn_shell(whkdrc.shell)?;

    let mut hkm = HotkeyManager::new();
    let pause_handle = hkm.pause_handle();

    if let Some(keys) = whkdrc.pause_binding {
        let mut mod_keys = vec![];

        let (mod_keys, vkey) = if keys.len() == 1 {
            (vec![], VKey::from_keyname(&keys[0])?)
        } else {
            let (trigger, mods) = keys.split_last().unwrap();
            let vkey = VKey::from_keyname(trigger)?;
            for m in mods {
                mod_keys.push(VKey::from_keyname(m)?);
            }

            (mod_keys, vkey)
        };

        if let Err(error) = hkm.register_pause_hotkey(vkey, mod_keys.as_slice(), move || {
            let current_state = if pause_handle.is_paused() {
                "paused"
            } else {
                "running"
            };

            println!("whkd is now {current_state}");

            if let Some(cmd) = &whkdrc.pause_hook {
                run_command(whkdrc.shell, cmd);
            }
        }) {
            eprintln!(
                "Unable to register pause hotkey '{mod_keys:?} + {vkey}' (error: {error}), ignoring this binding and continuing...",
            );
        }
    }

    // everything bound to the same hotkey is decided together when it is pressed, in file order
    let mut mapped: HashMap<TriggerId, Vec<HkmData>> = HashMap::new();
    let mut mouse: HashMap<(MouseInput, Modifiers), Vec<HkmData>> = HashMap::new();
    let mut gestures: HashMap<(GestureInput, Modifiers), Vec<HkmData>> = HashMap::new();
    for (_, app_bindings) in &whkdrc.app_bindings {
        for binding in app_bindings {
            let data = HkmData::try_from(binding)?;
            match (data.mouse_key(), data.gesture_key()) {
                (Some(key), _) => mouse.entry(key).or_default().push(data),
                (_, Some(key)) => gestures.entry(key).or_default().push(data),
                (None, None) => mapped.entry(data.id()).or_default().push(data),
            }
        }
    }

    // a plain binding for a hotkey that also has conditions is what runs when none of them match
    let mut plain = vec![];
    for binding in &whkdrc.bindings {
        let data = HkmData::try_from(binding)?;

        if let Some(key) = data.mouse_key() {
            mouse.entry(key).or_default().push(data);
            continue;
        }

        if let Some(key) = data.gesture_key() {
            gestures.entry(key).or_default().push(data);
            continue;
        }

        match mapped.get_mut(&data.id()) {
            Some(group) => group.push(data),
            None => plain.push(data),
        }
    }

    for (_, v) in mapped {
        let Trigger::Keyboard { mod_keys, vkey } = &v[0].trigger else {
            continue;
        };

        let (vkey, mod_keys) = (*vkey, mod_keys.clone());
        let shell = whkdrc.shell;

        hkm.register_hotkey(vkey, mod_keys.as_slice(), move || {
            if let Some(cmd) = select_command(&v) {
                run_command(shell, &cmd);
            }
        })?;
    }

    for data in plain {
        data.register(&mut hkm, whkdrc.shell)?;
    }

    // the touchpad is only listened to if there is a gesture to look out for
    if !gestures.is_empty() {
        let dispatcher = touchpad::dispatch::Dispatcher::new(
            gestures
                .into_iter()
                .map(|(key, entries)| (key, Arc::new(entries))),
            whkdrc.shell,
            |entries| select_command(entries),
            {
                let shell = whkdrc.shell;
                move |cmd| run_command(shell, cmd)
            },
        );

        if touchpad::device::find_touchpads().is_empty() {
            eprintln!(
                "No precision touchpad was found, so gesture bindings will not fire until one is connected"
            );
        }

        touchpad::device::start(
            dispatcher,
            hkm.pause_handle(),
            touchpad::recogniser::Thresholds {
                cooldown: Duration::from_millis(u64::from(
                    whkdrc
                        .gesture_cooldown
                        .unwrap_or(DEFAULT_GESTURE_COOLDOWN_MS),
                )),
                ..touchpad::recogniser::Thresholds::default()
            },
        )?;
    }

    // the hook that listens for the mouse is only started if there is something to listen for
    if !mouse.is_empty() {
        let shell = whkdrc.shell;

        mouse::start(
            mouse
                .into_iter()
                .map(|(key, entries)| (key, Arc::new(entries))),
            hkm.pause_handle(),
            Duration::from_millis(u64::from(
                whkdrc.wheel_cooldown.unwrap_or(DEFAULT_WHEEL_COOLDOWN_MS),
            )),
            false,
            move |entries| {
                if let Some(cmd) = select_command(&entries) {
                    run_command(shell, &cmd);
                }
            },
        )?;
    }

    hkm.event_loop();

    Ok(())
}
