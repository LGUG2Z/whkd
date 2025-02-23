use crate::parser::parser;
use crate::parser::HotkeyBinding;
use chumsky::Parser;
use color_eyre::eyre::eyre;
use color_eyre::eyre::Result;
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whkdrc {
    pub shell: Shell,
    pub app_bindings: Vec<(Vec<String>, Vec<HotkeyBinding>)>,
    pub bindings: Vec<HotkeyBinding>,
    pub pause: Option<Vec<String>>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Shell {
    Cmd,
    Powershell,
    Pwsh,
}

#[allow(clippy::fallible_impl_from)]
impl From<String> for Shell {
    fn from(value: String) -> Self {
        match value.as_str() {
            "pwsh" => Self::Pwsh,
            "powershell" => Self::Powershell,
            "cmd" => Self::Cmd,
            _ => panic!("unsupported shell"),
        }
    }
}

impl Display for Shell {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cmd => write!(f, "cmd"),
            Self::Powershell => write!(f, "powershell"),
            Self::Pwsh => write!(f, "pwsh"),
        }
    }
}

impl Whkdrc {
    pub fn load(path: &PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;

        parser()
            .parse(contents)
            .map_err(|error| eyre!("could not parse whkdrc: {:?}", error))
    }
}
