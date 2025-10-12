use std::collections::HashSet;
use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whkdrc {
    pub shell: Shell,
    pub app_bindings: Vec<(Vec<String>, Vec<HotkeyBinding>)>,
    pub bindings: Vec<HotkeyBinding>,
    pub pause_binding: Option<Vec<String>>,
    pub pause_hook: Option<String>,
}

impl Whkdrc {
    pub fn add_rwin_bindings(&mut self) {
        let initial_rwin_bindings: Vec<HashSet<&String>> = self
            .bindings
            .iter()
            .filter(|b| b.keys.iter().any(|k| k == "rwin"))
            .map(|b| HashSet::from_iter(b.keys.iter()))
            .collect();

        let mut rwin_bindings = vec![];
        for binding in &self.bindings {
            let Some(key_idx) = binding.keys.iter().position(|key| key == "win") else {
                continue;
            };

            let mut rwin_binding = binding.clone();
            rwin_binding.keys[key_idx] = "rwin".to_string();

            // skip any hotkeys that are already specified
            if initial_rwin_bindings
                .iter()
                .any(|b| b == &HashSet::from_iter(rwin_binding.keys.iter()))
            {
                continue;
            }

            rwin_bindings.push(rwin_binding);
        }
        self.bindings.extend(rwin_bindings);

        let initial_rwin_app_bindings: Vec<HashSet<&String>> = self
            .app_bindings
            .iter()
            .filter(|b| b.0.iter().any(|k| k == "rwin"))
            .map(|b| HashSet::from_iter(b.0.iter()))
            .collect();

        let mut rwin_app_bindings = vec![];
        for binding in &self.app_bindings {
            let Some(key_idx) = binding.0.iter().position(|key| key == "win") else {
                continue;
            };

            let mut rwin_binding = binding.clone();
            rwin_binding.0[key_idx] = "rwin".to_string();
            for app_binding in &mut rwin_binding.1 {
                app_binding.keys[key_idx] = "rwin".to_string();
            }

            // skip any hotkeys that are already specified
            if initial_rwin_app_bindings
                .iter()
                .any(|b| b == &HashSet::from_iter(rwin_binding.0.iter()))
            {
                continue;
            }

            rwin_app_bindings.push(rwin_binding);
        }
        self.app_bindings.extend(rwin_app_bindings);
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub keys: Vec<String>,
    pub command: String,
    pub process_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hb(keys: &str, cmd: &str) -> HotkeyBinding {
        HotkeyBinding {
            keys: keys.split(" + ").map(String::from).collect(),
            command: String::from(cmd),
            process_name: None,
        }
    }

    #[test]
    fn test_add_rwin_binding() {
        let mut config = Whkdrc {
            bindings: vec![hb("win + j", "win + j"), hb("alt + j", "alt + j")],
            shell: Shell::Pwsh,
            app_bindings: vec![],
            pause_binding: None,
            pause_hook: None,
        };

        config.add_rwin_bindings();

        let expected = vec![
            hb("win + j", "win + j"),
            hb("alt + j", "alt + j"),
            hb("rwin + j", "win + j"),
        ];

        assert_eq!(config.bindings, expected);
    }

    #[test]
    fn test_use_provided_rwin() {
        let mut config = Whkdrc {
            bindings: vec![
                hb("win + j", "win + j"),
                hb("j + rwin", "j + rwin"),
                hb("alt + j", "alt + j"),
            ],
            shell: Shell::Pwsh,
            app_bindings: vec![],
            pause_binding: None,
            pause_hook: None,
        };

        config.add_rwin_bindings();

        let expected = vec![
            hb("win + j", "win + j"),
            hb("j + rwin", "j + rwin"),
            hb("alt + j", "alt + j"),
        ];

        assert_eq!(config.bindings, expected);
    }
}
