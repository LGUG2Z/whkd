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
        let mut rwin_bindings = vec![];
        for binding in &mut self.bindings {
            if let Some(i) = binding.keys.iter().position(|key| key == "win") {
                let mut rwin_binding = binding.clone();
                rwin_binding.keys[i] = "rwin".to_string();
                rwin_bindings.push(rwin_binding);
            }
        }
        self.bindings.extend(rwin_bindings);

        let mut rwin_app_bindings = vec![];
        for binding in &mut self.app_bindings {
            if let Some(i) = binding.0.iter().position(|key| key == "win") {
                let mut rwin_binding = binding.clone();
                rwin_binding.0[i] = "rwin".to_string();
                for app_binding in &mut rwin_binding.1 {
                    app_binding.keys[i] = "rwin".to_string();
                }
                rwin_app_bindings.push(rwin_binding);
            }
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
