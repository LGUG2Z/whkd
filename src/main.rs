#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::redundant_pub_crate)]

use crate::parser::HotkeyBinding;
use crate::whkdrc::Shell;
use crate::whkdrc::Whkdrc;
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
use win_hotkeys::error::WHKError;
use win_hotkeys::keys::ModKey;
use win_hotkeys::keys::VKey;
use win_hotkeys::HotkeyManager;

mod parser;
mod whkdrc;

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
        Whkdrc::load(&home).unwrap_or_else(|_| panic!("could not load whkdrc from {home:?}"))
    };
    static ref SESSION_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);
}

#[derive(Debug, Clone)]
pub struct HkmData {
    pub mod_keys: Vec<ModKey>,
    pub vkey: VKey,
    pub command: String,
    pub process_name: Option<String>,
}

impl HkmData {
    pub fn register(&self, hkm: &mut HotkeyManager<()>, shell: Shell) -> Result<()> {
        let cmd = self.command.clone();

        if let Err(error) = hkm.register_hotkey(self.vkey, self.mod_keys.as_slice(), move || {
            if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                if matches!(shell, Shell::Pwsh | Shell::Powershell) {
                    println!("{cmd}");
                }

                writeln!(session_stdin, "{cmd}").expect("failed to execute command");
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
                mod_keys.push(ModKey::from_keyname(m)?);
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

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    let whkdrc = cli.config.map_or_else(
        || WHKDRC.clone(),
        |config| {
            Whkdrc::load(&config)
                .unwrap_or_else(|_| panic!("could not load whkdrc from {config:?}"))
        },
    );

    let shell_binary = whkdrc.shell.to_string();

    match whkdrc.shell {
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

    let mut hkm = HotkeyManager::new();

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
            if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                for e in &v {
                    let cmd = &e.command;
                    if let Some(proc) = &e.process_name {
                        match active_win_pos_rs::get_active_window() {
                            Ok(window) => {
                                if window.app_name == *proc {
                                    if matches!(whkdrc.shell, Shell::Pwsh | Shell::Powershell) {
                                        println!("{cmd}");
                                    }

                                    writeln!(session_stdin, "{cmd}")
                                        .expect("failed to execute command");
                                }
                            }
                            Err(error) => {
                                dbg!(error);
                            }
                        }
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
