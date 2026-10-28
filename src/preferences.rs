use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
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
    save_in(&dir, value, name)
}
fn save_in(dir: &Path, value: &impl Serialize, name: &str) -> Result<(), String> {
    if name.is_empty() || Path::new(name).file_name().and_then(|s| s.to_str()) != Some(name) {
        return Err("Invalid settings filename".into());
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Configuration exceeds the 4 MiB safety limit".into());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let info = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    // SAFETY: getuid has no preconditions.
    if !info.is_dir() || info.uid() != unsafe { libc::getuid() } {
        return Err("Settings directory must be owned by this user and not be a symlink".into());
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        ".{name}.{}-{}.new",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, dir.join(name))?;
        fs::File::open(dir)?.sync_all()
    })();
    let _ = fs::remove_file(temp);
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod security_tests {
    use super::*;
    #[test]
    fn settings_are_private_and_do_not_follow_temp_or_destination_symlinks() {
        let dir =
            std::env::temp_dir().join(format!("laptop-settings-security-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let victim = dir.join("victim");
        fs::write(&victim, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("settings.json.new")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join("settings.json")).unwrap();
        save_in(&dir, &serde_json::json!({"safe":true}), "settings.json").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(dir.join("settings.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(save_in(&dir, &true, "../escape.json").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
