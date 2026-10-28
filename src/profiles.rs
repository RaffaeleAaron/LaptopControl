use crate::{Config, Mode};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub gpu_poll_seconds: u64,
    #[serde(default)]
    pub gpu_usage_in_gaming: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            gpu_poll_seconds: 5,
            gpu_usage_in_gaming: false,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=300).contains(&self.gpu_poll_seconds) {
            return Err("dGPU polling interval must be 1–300 seconds".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Profile {
    #[default]
    Firmware,
    Office,
    Gaming,
}
impl Profile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Firmware => "Firmware",
            Self::Office => "Office",
            Self::Gaming => "Gaming",
        }
    }
    pub fn index(self) -> Option<usize> {
        match self {
            Self::Firmware => None,
            Self::Office => Some(0),
            Self::Gaming => Some(1),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub selected: Profile,
    pub office: Config,
    pub gaming: Config,
    #[serde(default)]
    pub settings: Settings,
}
impl Default for Profiles {
    fn default() -> Self {
        let office = Config {
            mode: Mode::Curve,
            ..Config::default()
        };
        let mut gaming = office.clone();
        for fan in &mut gaming.fans {
            fan.curve = vec![[30., 30.], [45., 45.], [60., 65.], [75., 90.], [85., 100.]];
        }
        Self {
            selected: Profile::Firmware,
            office,
            gaming,
            settings: Settings::default(),
        }
    }
}
impl Profiles {
    pub fn config(&self, profile: Profile) -> Config {
        match profile {
            Profile::Firmware => {
                let mut c = self.office.clone();
                c.mode = Mode::Firmware;
                c
            }
            Profile::Office => self.office.clone(),
            Profile::Gaming => self.gaming.clone(),
        }
    }
    pub fn active(&self) -> Config {
        self.config(self.selected)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.settings.validate()?;
        for (name, c) in [("Office", &self.office), ("Gaming", &self.gaming)] {
            c.validate().map_err(|e| format!("{name}: {e}"))?;
            if c.mode != Mode::Curve {
                return Err(format!("{name} must use independent controls"));
            }
        }
        Ok(())
    }
    pub fn save_profile(&mut self, profile: Profile, config: Config) -> Result<(), String> {
        if profile == Profile::Firmware {
            return Err("Firmware uses the laptop's built-in controller".into());
        }
        config.validate()?;
        if config.mode != Mode::Curve {
            return Err("Use independent controls for Office and Gaming".into());
        }
        match profile {
            Profile::Office => self.office = config,
            Profile::Gaming => self.gaming = config,
            Profile::Firmware => unreachable!(),
        };
        self.selected = profile;
        Ok(())
    }
    pub fn from_legacy(config: Config) -> Self {
        let selected = if config.mode == Mode::Firmware {
            Profile::Firmware
        } else {
            Profile::Office
        };
        let mut office = config;
        office.mode = Mode::Curve;
        Self {
            selected,
            office,
            ..Self::default()
        }
    }
    /// Missing profiles migrate the existing saved configuration, preserving its active mode.
    pub fn load(directory: &Path) -> Result<Self, String> {
        let path = directory.join("profiles.json");
        let profiles = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid saved profiles: {e}"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::read(directory.join("config.json")) {
                    Ok(bytes) => Self::from_legacy(
                        serde_json::from_slice(&bytes)
                            .map_err(|e| format!("Invalid saved configuration: {e}"))?,
                    ),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
                    Err(e) => return Err(e.to_string()),
                }
            }
            Err(e) => return Err(e.to_string()),
        };
        profiles.validate()?;
        Ok(profiles)
    }
    pub fn persist(&self, directory: &Path) -> io::Result<()> {
        self.validate().map_err(io::Error::other)?;
        let temp = directory.join("profiles.json.new");
        let file = fs::File::create(&temp)?;
        serde_json::to_writer_pretty(&file, self)?;
        file.sync_all()?;
        fs::rename(temp, directory.join("profiles.json"))?;
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_profile_and_selection_restore() {
        let dir = std::env::temp_dir().join(format!("laptop-profiles-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut p = Profiles::default();
        let mut office = p.office.clone();
        office.fans[0].mode = Mode::Manual;
        office.fans[0].manual = 37;
        p.save_profile(Profile::Office, office).unwrap();
        p.persist(&dir).unwrap();
        let loaded = Profiles::load(&dir).unwrap();
        assert_eq!(loaded.selected, Profile::Office);
        assert_eq!(loaded.active().fans[0].manual, 37);
        p.selected = Profile::Gaming;
        p.persist(&dir).unwrap();
        assert_eq!(Profiles::load(&dir).unwrap().selected, Profile::Gaming);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn profiles_are_independent() {
        let mut p = Profiles::default();
        let gaming = p.gaming.clone();
        let mut office = p.office.clone();
        office.fans[1].unavailable = 40;
        p.save_profile(Profile::Office, office).unwrap();
        assert_eq!(p.gaming, gaming);
        assert_eq!(p.office.fans[1].unavailable, 40);
    }
    #[test]
    fn legacy_active_config_is_preserved() {
        let c = Config {
            mode: Mode::Curve,
            ..Config::default()
        };
        let p = Profiles::from_legacy(c.clone());
        assert_eq!(p.selected, Profile::Office);
        assert_eq!(p.active(), c);
    }
    #[test]
    fn existing_profiles_default_to_safe_polling_settings() {
        let mut json = serde_json::to_value(Profiles::default()).unwrap();
        json.as_object_mut().unwrap().remove("settings");
        let p: Profiles = serde_json::from_value(json).unwrap();
        assert_eq!(p.settings.gpu_poll_seconds, 5);
        assert!(!p.office.uses_nvidia_smi());
        assert!(!p.gaming.uses_nvidia_smi());
        assert!(p.validate().is_ok());
    }
}
