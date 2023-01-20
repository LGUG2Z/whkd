#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::redundant_pub_crate)]

use crate::parser::HotkeyBinding;
use crate::whkdrc::Shell;
use crate::whkdrc::Whkdrc;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::io::Write;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use windows_hotkeys::error::HkError;
use windows_hotkeys::keys::ModKey;
use windows_hotkeys::keys::VKey;
use windows_hotkeys::HotkeyManager;

mod parser;
mod whkdrc;

lazy_static! {
    static ref WHKDRC: Whkdrc = {
        let mut home = dirs::home_dir().expect("no home directory found");
        home.push(".config");
        home.push("whkdrc");

        Whkdrc::load(&home).expect("could not load whkdrc")
    };
    static ref SESSION_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);
}

#[derive(Debug)]
pub struct HkmData {
    pub mod_keys: Vec<ModKey>,
    pub vkey: VKey,
    pub command: String,
}

impl HkmData {
    pub fn register(&self, hkm: &mut HotkeyManager<()>) -> Result<()> {
        let cmd = self.command.clone();

        hkm.register(self.vkey, self.mod_keys.as_slice(), move || {
            if let Some(session_stdin) = SESSION_STDIN.lock().as_mut() {
                if matches!(WHKDRC.shell, Shell::Pwsh | Shell::Powershell) {
                    println!("{cmd}");
                }

                writeln!(session_stdin, "{cmd}").expect("failed to execute command");
            }
        })?;

        Ok(())
    }
}

impl TryFrom<&HotkeyBinding> for HkmData {
    type Error = HkError;

    fn try_from(value: &HotkeyBinding) -> Result<Self, Self::Error> {
        let (trigger, mods) = value.keys.split_last().unwrap();
        let mut mod_keys = vec![];
        let vkey = VKey::from_keyname(trigger)?;
        for m in mods {
            mod_keys.push(ModKey::from_keyname(m)?);
        }

        Ok(Self {
            mod_keys,
            vkey,
            command: value.command.clone(),
        })
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let shell_binary = WHKDRC.shell.to_string();

    match WHKDRC.shell {
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

    for binding in &WHKDRC.bindings {
        HkmData::try_from(binding)?.register(&mut hkm)?;
    }

    hkm.event_loop();

    Ok(())
}
