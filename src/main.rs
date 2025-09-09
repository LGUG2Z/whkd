#![warn(clippy::all)]
#![allow(clippy::missing_errors_doc, clippy::redundant_pub_crate)]

use clap::Parser;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use whkd_core::HotkeyBinding;
use whkd_core::Shell;
use whkd_core::Whkdrc;
use win_hotkeys::error::WHKError;
use win_hotkeys::HotkeyManager;
use win_hotkeys::VKey;

lazy_static! {
    static ref WHKDRC: Whkdrc = {
        // config file defaults to `~/.config/whkdrc`, or `<WHKD_CONFIG_HOME>/whkdrc`
        let mut home  = std::env::var("WHKD_CONFIG_HOME").map_or_else(
            |_| dirs::home_dir().expect("no home directory found").join(".config"),
            |home_path| {
                let home = PathBuf::from(&home_path);

                if home.as_path().is_dir() {
                    home
                } else {
                    panic!(
                        "$Env:WHKD_CONFIG_HOME is set to '{home_path}', which is not a valid directory",
                    );
                }
            },
        );
        home.push("whkdrc");
        whkd_parser::load(&home).unwrap_or_else(|_| panic!("could not load whkdrc from {home:?}"))
    };
    static ref SESSION_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);
}

#[derive(Debug, Clone)]
pub struct HkmData {
    pub mod_keys: Vec<VKey>,
    pub vkey: VKey,
    pub command: String,
    pub process_name: Option<String>,
}

impl HkmData {
    pub fn register(&self, hkm: &mut HotkeyManager<()>, shell: Shell) -> Result<()> {
        let cmd = self.command.clone();

        if let Err(error) = hkm.register_hotkey(self.vkey, self.mod_keys.as_slice(), move || {
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
        }) {
            eprintln!(
                "Unable to bind '{:?} + {}' to '{}' (error: {error}), ignoring this binding and continuing...",
                self.mod_keys, self.vkey, self.command
            );
        }

        Ok(())
    }
}

impl TryFrom<&HotkeyBinding> for HkmData {
    type Error = WHKError;

    fn try_from(value: &HotkeyBinding) -> Result<Self, Self::Error> {
        let mut mod_keys = vec![];

        let (mod_keys, vkey) = if value.keys.len() == 1 {
            (vec![], VKey::from_keyname(&value.keys[0])?)
        } else {
            let (trigger, mods) = value.keys.split_last().unwrap();
            let vkey = VKey::from_keyname(trigger)?;
            for m in mods {
                mod_keys.push(VKey::from_keyname(m)?);
            }

            (mod_keys, vkey)
        };

        Ok(Self {
            mod_keys,
            vkey,
            command: value.command.clone(),
            process_name: value.process_name.clone(),
        })
    }
}

#[derive(Parser)]
#[clap(author, about, version)]
struct Cli {
    /// Path to whkdrc
    #[clap(action, short, long)]
    config: Option<PathBuf>,
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

    let whkdrc = cli.config.map_or_else(
        || WHKDRC.clone(),
        |config| {
            whkd_parser::load(&config)
                .unwrap_or_else(|_| panic!("could not load whkdrc from {config:?}"))
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
                let mut retry_with_new_session = false;
                if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                    if matches!(whkdrc.shell, Shell::Pwsh | Shell::Powershell) {
                        println!("{cmd}");
                    }

                    if writeln!(session_stdin, "{cmd}").is_err() {
                        retry_with_new_session = true;
                    }
                }

                if retry_with_new_session && spawn_shell(whkdrc.shell).is_ok() {
                    if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                        if matches!(whkdrc.shell, Shell::Pwsh | Shell::Powershell) {
                            println!("{cmd}");
                        }

                        if writeln!(session_stdin, "{cmd}").is_err() {
                            eprintln!("Unable to write to stdin session");
                        }
                    }
                }
            }
        }) {
            eprintln!(
                "Unable to register pause hotkey '{mod_keys:?} + {vkey}' (error: {error}), ignoring this binding and continuing...",
            );
        }
    }

    let mut mapped = HashMap::new();
    for (keys, app_bindings) in &whkdrc.app_bindings {
        for binding in app_bindings {
            let data = HkmData::try_from(binding)?;
            mapped
                .entry(keys.join("+"))
                .or_insert_with(Vec::new)
                .push(data);
        }
    }

    for (_, v) in mapped {
        let vkey = v[0].vkey;
        let mod_keys = v[0].mod_keys.as_slice();

        let v = v.clone();
        hkm.register_hotkey(vkey, mod_keys, move || {
            let mut retry_with_new_session = false;

            if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                let app_name = active_win_pos_rs::get_active_window()
                    .unwrap_or_default()
                    .app_name;

                let mut matched_cmd = None;
                let mut default_cmd = None;

                for e in &v {
                    let cmd = &e.command;

                    if let Some(proc) = &e.process_name {
                        if *proc == "Default" {
                            default_cmd = Some(cmd.clone());
                        }

                        if app_name == *proc {
                            matched_cmd = Some(cmd.clone());
                        }
                    }
                }

                match (matched_cmd, default_cmd) {
                    (None, Some(cmd)) | (Some(cmd), _) if cmd != "Ignore" => {
                        if matches!(whkdrc.shell, Shell::Pwsh | Shell::Powershell) {
                            println!("{cmd}");
                        }

                        if writeln!(session_stdin, "{cmd}").is_err() {
                            retry_with_new_session = true;
                        }
                    }
                    (_, _) => {}
                }
            }

            if retry_with_new_session && spawn_shell(whkdrc.shell).is_ok() {
                if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                    let app_name = active_win_pos_rs::get_active_window()
                        .unwrap_or_default()
                        .app_name;

                    let mut matched_cmd = None;
                    let mut default_cmd = None;

                    for e in &v {
                        let cmd = &e.command;

                        if let Some(proc) = &e.process_name {
                            if *proc == "Default" {
                                default_cmd = Some(cmd.clone());
                            }

                            if app_name == *proc {
                                matched_cmd = Some(cmd.clone());
                            }
                        }
                    }

                    match (matched_cmd, default_cmd) {
                        (None, Some(cmd)) | (Some(cmd), _) if cmd != "Ignore" => {
                            if matches!(whkdrc.shell, Shell::Pwsh | Shell::Powershell) {
                                println!("{cmd}");
                            }

                            if writeln!(session_stdin, "{cmd}").is_err() {
                                eprintln!("Unable to write to stdin session");
                            }
                        }
                        (_, _) => {}
                    }
                }
            }
        })?;
    }

    for binding in &whkdrc.bindings {
        HkmData::try_from(binding)?.register(&mut hkm, whkdrc.shell)?;
    }

    hkm.event_loop();

    Ok(())
}
