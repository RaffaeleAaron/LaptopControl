use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub accent: Option<[u8; 3]>,
    pub opacity: u8,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::Kde,
            accent: None,
            opacity: 85,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Cards {
    pub system: bool,
    pub power: bool,
    pub usage: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Preferences {
    pub appearance: Appearance,
    pub cards: Cards,
    pub sidebar_collapsed: bool,
}
pub fn directory() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("laptop-control")
}
pub fn load() -> Preferences {
    let mut p: Preferences = fs::read(directory().join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| Preferences {
            appearance: Appearance {
                theme: crate::theme::load(),
                ..Appearance::default()
            },
            ..Preferences::default()
        });
    p.appearance.opacity = p.appearance.opacity.clamp(30, 100);
    p
}
pub fn save(value: &impl Serialize, name: &str) -> Result<(), String> {
    let dir = directory();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = dir.join(format!("{name}.new"));
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temp, dir.join(name)).map_err(|e| e.to_string())
}
