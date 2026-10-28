use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};
pub mod curve;
pub mod hardware;
pub mod profiles;
pub use profiles::{Profile, Profiles, Settings};
pub mod averaging;
pub mod smoothing;
pub mod telemetry;
pub const SOCKET: &str = "/run/raf-fans/control.sock";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Mode {
    #[default]
    Firmware,
    Curve,
    Manual,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sensor {
    #[default]
    Cpu,
    Gpu,
    NvidiaSmi,
}
impl Sensor {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Gpu, Self::NvidiaSmi];
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU temperature",
            Self::Gpu => "dGPU temperature · hwmon",
            Self::NvidiaSmi => "dGPU temperature · nvidia-smi",
        }
    }
    pub fn temperature(self, t: &Temperatures) -> Option<f32> {
        match self {
            Self::Cpu => t.cpu,
            Self::Gpu => t.gpu,
            Self::NvidiaSmi => t.gpu_smi,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredFan")]
pub struct FanConfig {
    pub mode: Mode,
    pub sensor: Sensor,
    pub smoothing: smoothing::Smoothing,
    pub average_seconds: u16,
    pub manual: u8,
    pub curve: Vec<[f32; 2]>,
    #[serde(default)]
    pub unavailable: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFan {
    mode: Mode,
    manual: u8,
    curve: Vec<[f32; 2]>,
    #[serde(default)]
    unavailable: u8,
    #[serde(default)]
    sensor: Option<Sensor>,
    #[serde(default)]
    smoothing: smoothing::Smoothing,
    #[serde(default)]
    average_seconds: u16,
}
impl StoredFan {
    fn into_fan(self, index: usize) -> FanConfig {
        FanConfig {
            mode: self.mode,
            smoothing: self.smoothing,
            average_seconds: self.average_seconds,
            manual: self.manual,
            curve: self.curve,
            unavailable: self.unavailable,
            sensor: self
                .sensor
                .unwrap_or(if index == 0 { Sensor::Cpu } else { Sensor::Gpu }),
        }
    }
}
impl From<StoredFan> for FanConfig {
    fn from(stored: StoredFan) -> Self {
        stored.into_fan(0)
    }
}
impl Default for FanConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Curve,
            sensor: Sensor::Cpu,
            smoothing: smoothing::Smoothing::default(),
            average_seconds: 0,
            manual: 50,
            curve: vec![[35., 0.], [50., 30.], [65., 50.], [75., 70.], [85., 100.]],
            unavailable: 0,
        }
    }
}
impl FanConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.smoothing.validate()?;
        if self.average_seconds > 60 {
            return Err("Temperature averaging must be 0–60 seconds".into());
        }
        if self.mode == Mode::Firmware {
            return Err("Firmware mode is shared by both fans".into());
        }
        if self.manual > 100 || self.unavailable > 100 {
            return Err("Fan speed must be 0–100%".into());
        }
        if !(2..=12).contains(&self.curve.len()) {
            return Err("Use 2–12 curve points".into());
        }
        for (i, p) in self.curve.iter().enumerate() {
            if !p[0].is_finite()
                || !p[1].is_finite()
                || !(20. ..=90.).contains(&p[0])
                || !(0. ..=100.).contains(&p[1])
            {
                return Err("Curve points require 20–90°C and 0–100%".into());
            }
            if i > 0 && (p[0] <= self.curve[i - 1][0] || p[1] < self.curve[i - 1][1]) {
                return Err("Temperatures must increase and speeds must not decrease".into());
            }
        }
        if self.curve.last().unwrap()[1] != 100. {
            return Err("Last point must reach 100%".into());
        }
        Ok(())
    }
    pub fn speed(&self, temp: f32) -> u8 {
        for pair in self.curve.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            if temp <= b[0] {
                let t = ((temp - a[0]) / (b[0] - a[0])).clamp(0., 1.);
                return (a[1] + t * (b[1] - a[1])).ceil() as u8;
            }
        }
        100
    }
    fn target(&self, temp: Option<f32>) -> u8 {
        // A readable hot sensor overrides manual settings; absent GPU telemetry is user-configurable.
        if self.mode == Mode::Manual {
            self.manual
        } else if temp.is_some_and(|t| t >= 90.) {
            100
        } else {
            temp.map(|t| self.speed(t)).unwrap_or(self.unavailable)
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    pub mode: Mode,
    pub fans: [FanConfig; 2],
}
impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Firmware,
            fans: std::array::from_fn(|i| FanConfig {
                sensor: if i == 0 { Sensor::Cpu } else { Sensor::Gpu },
                ..FanConfig::default()
            }),
        }
    }
}
impl Config {
    pub fn uses_nvidia_smi(&self) -> bool {
        self.mode != Mode::Firmware
            && self
                .fans
                .iter()
                .any(|f| f.mode == Mode::Curve && f.sensor == Sensor::NvidiaSmi)
    }
    pub fn validate(&self) -> Result<(), String> {
        for (i, fan) in self.fans.iter().enumerate() {
            fan.validate()
                .map_err(|e| format!("{}: {e}", if i == 0 { "CPU" } else { "dGPU" }))?;
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stored {
            mode: Mode,
            #[serde(default)]
            fans: Option<[StoredFan; 2]>,
            #[serde(default)]
            manual: Option<[u8; 2]>,
            #[serde(default)]
            curve: Option<Vec<[f32; 2]>>,
        }
        let stored = Stored::deserialize(deserializer)?;
        let fans = stored
            .fans
            .map(|fans| {
                let [cpu, gpu] = fans;
                [cpu.into_fan(0), gpu.into_fan(1)]
            })
            .unwrap_or_else(|| {
                std::array::from_fn(|i| FanConfig {
                    mode: if stored.mode == Mode::Manual {
                        Mode::Manual
                    } else {
                        Mode::Curve
                    },
                    manual: stored.manual.unwrap_or([50, 50])[i],
                    curve: stored
                        .curve
                        .clone()
                        .unwrap_or_else(|| FanConfig::default().curve),
                    unavailable: 0,
                    smoothing: smoothing::Smoothing::default(),
                    average_seconds: 0,
                    sensor: if i == 0 { Sensor::Cpu } else { Sensor::Gpu },
                })
            });
        Ok(Self {
            mode: if stored.mode == Mode::Manual {
                Mode::Curve
            } else {
                stored.mode
            },
            fans,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Temperatures {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    #[serde(default)]
    pub gpu_smi: Option<f32>,
    #[serde(default)]
    pub gpu_usage: Option<f32>,
    pub gpu_expected: bool,
    #[serde(default)]
    pub gpu_state: String,
}
impl Temperatures {
    pub fn physical_temperature(&self, fan: usize) -> Option<f32> {
        if fan == 0 {
            self.cpu
        } else {
            match (self.gpu, self.gpu_smi) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }
        }
    }
}
pub fn read_temperatures(root: &Path) -> Temperatures {
    let mut result = Temperatures::default();
    if let Ok(dirs) = fs::read_dir(root) {
        for dir in dirs.flatten() {
            let path = dir.path();
            let name = fs::read_to_string(path.join("name")).unwrap_or_default();
            let cpu = name.trim() == "coretemp";
            let gpu = matches!(name.trim(), "amdgpu");
            if !cpu && !gpu {
                continue;
            }
            if gpu {
                result.gpu_expected = true;
            }
            if let Ok(files) = fs::read_dir(&path) {
                for file in files.flatten() {
                    let n = file.file_name();
                    let n = n.to_string_lossy();
                    if !n.starts_with("temp") || !n.ends_with("_input") {
                        continue;
                    }
                    let temp = fs::read_to_string(file.path())
                        .ok()
                        .and_then(|s| s.trim().parse::<f32>().ok())
                        .map(|n| n / 1000.)
                        .filter(|n| n.is_finite() && (1. ..=125.).contains(n));
                    let slot = if cpu {
                        &mut result.cpu
                    } else {
                        &mut result.gpu
                    };
                    if let Some(t) = temp {
                        *slot = Some(slot.unwrap_or(t).max(t));
                    }
                }
            }
        }
    }
    result
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub config: Config,
    pub temperatures: Temperatures,
    pub requested: Option<[u8; 2]>,
    #[serde(default)]
    pub control_temperatures: [Option<f32>; 2],
    pub raw_fans: [Option<i32>; 2],
    pub message: String,
    pub simulated: bool,
    #[serde(default)]
    pub sample_age_ms: u64,
    #[serde(default)]
    pub profiles: Profiles,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            config: Config::default(),
            temperatures: Temperatures::default(),
            requested: None,
            control_temperatures: [None; 2],
            raw_fans: [None, None],
            message: "Connecting…".into(),
            simulated: false,
            sample_age_ms: 0,
            profiles: Profiles::default(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Status,
    Apply(Config),
    SelectProfile(Profile),
    SaveProfile { profile: Profile, config: Config },
    SaveSettings(Settings),
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub status: Status,
    pub error: Option<String>,
}
pub fn read_request(stream: &UnixStream) -> io::Result<Request> {
    let mut line = String::new();
    BufReader::new(stream.take(8193)).read_line(&mut line)?;
    if line.len() > 8192 || !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid request length",
        ));
    }
    serde_json::from_str(&line).map_err(io::Error::other)
}
pub fn exchange(socket: &str, request: Request) -> Result<Status, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|e| format!("Service unavailable: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{}", serde_json::to_string(&request).unwrap()).map_err(|e| e.to_string())?;
    let reply: Reply = serde_json::from_reader(stream.take(65536)).map_err(|e| e.to_string())?;
    match reply.error {
        Some(e) => Err(e),
        None => Ok(reply.status),
    }
}
/// CPU telemetry is mandatory. GPU telemetry never causes a global firmware fallback.
/// Pure desired targets; the daemon applies optional smoothing to normal curve changes.
pub fn target(config: &Config, temperatures: &Temperatures) -> Option<[u8; 2]> {
    target_with_sources(
        config,
        temperatures,
        std::array::from_fn(|i| config.fans[i].sensor.temperature(temperatures)),
    )
}
pub fn target_with_sources(
    config: &Config,
    temperatures: &Temperatures,
    sources: [Option<f32>; 2],
) -> Option<[u8; 2]> {
    if config.mode == Mode::Firmware {
        return None;
    }
    temperatures.cpu?;
    Some(std::array::from_fn(|i| {
        let fan = &config.fans[i];
        // Physical thermal protection remains tied to the hardware being cooled,
        // even when the user chooses another sensor for the curve.
        let physical = temperatures.physical_temperature(i);
        if physical.is_some_and(|t| t >= 90.)
            || (fan.mode == Mode::Curve
                && fan
                    .sensor
                    .temperature(temperatures)
                    .is_some_and(|t| t >= 90.))
        {
            100
        } else {
            fan.target(sources[i])
        }
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn temperatures(cpu: Option<f32>, gpu: Option<f32>) -> Temperatures {
        Temperatures {
            cpu,
            gpu,
            ..Temperatures::default()
        }
    }
    fn custom() -> Config {
        Config {
            mode: Mode::Curve,
            ..Config::default()
        }
    }
    #[test]
    fn interpolation() {
        let f = FanConfig::default();
        assert_eq!(f.speed(0.), 0);
        assert_eq!(f.speed(57.5), 40);
        assert_eq!(f.speed(110.), 100);
    }
    #[test]
    fn rejects_invalid_configuration() {
        let mut c = custom();
        c.fans[0].curve[1][0] = 34.;
        assert!(c.validate().is_err());
        c = custom();
        c.fans[1].manual = 101;
        assert!(c.validate().is_err());
        c = custom();
        c.fans[0].curve[0][1] = f32::NAN;
        assert!(c.validate().is_err());
        c = custom();
        c.fans[1].unavailable = 101;
        assert!(c.validate().is_err());
    }
    #[test]
    fn independent_sensors() {
        let c = custom();
        assert_eq!(
            target(&c, &temperatures(Some(50.), Some(75.))),
            Some([30, 70])
        );
    }
    #[test]
    fn either_fan_can_follow_any_selected_source() {
        let mut c = custom();
        let t = Temperatures {
            cpu: Some(50.),
            gpu: Some(65.),
            gpu_smi: Some(75.),
            ..Temperatures::default()
        };
        c.fans[0].sensor = Sensor::NvidiaSmi;
        c.fans[1].sensor = Sensor::Cpu;
        assert_eq!(target(&c, &t), Some([70, 30]));
        c.fans[0].sensor = Sensor::Gpu;
        c.fans[1].sensor = Sensor::NvidiaSmi;
        assert_eq!(target(&c, &t), Some([50, 70]));
        c.fans[0].sensor = Sensor::NvidiaSmi;
        let sleeping = Temperatures {
            cpu: Some(50.),
            ..Temperatures::default()
        };
        c.fans[0].unavailable = 35;
        assert_eq!(target(&c, &sleeping), Some([35, 0]));
        assert!(c.uses_nvidia_smi());
        c.mode = Mode::Firmware;
        assert!(!c.uses_nvidia_smi());
        c.mode = Mode::Curve;
        for fan in &mut c.fans {
            fan.mode = Mode::Manual;
        }
        assert!(!c.uses_nvidia_smi());
    }
    #[test]
    fn cross_sensor_curves_keep_physical_thermal_protection() {
        let mut c = custom();
        c.fans[0].sensor = Sensor::NvidiaSmi;
        c.fans[1].sensor = Sensor::Cpu;
        let t = Temperatures {
            cpu: Some(50.),
            gpu_smi: Some(91.),
            ..Temperatures::default()
        };
        assert_eq!(target(&c, &t), Some([100, 100]));
        let t = Temperatures {
            cpu: Some(91.),
            gpu_smi: Some(50.),
            ..Temperatures::default()
        };
        assert_eq!(target(&c, &t), Some([100, 100]));
    }
    #[test]
    fn old_independent_profiles_preserve_sources_without_enabling_smi() {
        let mut json = serde_json::to_value(custom()).unwrap();
        for fan in json["fans"].as_array_mut().unwrap() {
            fan.as_object_mut().unwrap().remove("sensor");
            fan.as_object_mut().unwrap().remove("smoothing");
        }
        let c: Config = serde_json::from_value(json).unwrap();
        assert_eq!(c.fans[0].sensor, Sensor::Cpu);
        assert_eq!(c.fans[1].sensor, Sensor::Gpu);
        assert!(!c.uses_nvidia_smi());
        assert_eq!(
            target(&c, &temperatures(Some(50.), Some(75.))),
            Some([30, 70])
        );
    }
    #[test]
    fn gpu_sleep_and_unreadable_states_never_reset_cpu() {
        let c = custom();
        for state in ["D0", "Unknown", "D3hot", "D3cold", "Not detected"] {
            let t = Temperatures {
                cpu: Some(50.),
                gpu: None,
                gpu_expected: true,
                gpu_state: state.into(),
                ..Temperatures::default()
            };
            assert_eq!(target(&c, &t), Some([30, 0]));
        }
    }
    #[test]
    fn gpu_missing_fallback_is_user_selected() {
        let mut c = custom();
        c.fans[1].unavailable = 40;
        assert_eq!(target(&c, &temperatures(Some(50.), None)), Some([30, 40]));
        c.fans[1].unavailable = 0;
        assert_eq!(target(&c, &temperatures(Some(50.), None)), Some([30, 0]));
    }
    #[test]
    fn manual_gpu_zero_without_sensor() {
        let mut c = custom();
        c.fans[1].mode = Mode::Manual;
        c.fans[1].manual = 0;
        assert_eq!(target(&c, &temperatures(Some(50.), None)), Some([30, 0]));
    }
    #[test]
    fn thermal_overrides_are_per_channel() {
        let mut c = custom();
        for fan in &mut c.fans {
            fan.mode = Mode::Manual;
            fan.manual = 0;
        }
        assert_eq!(target(&c, &temperatures(Some(91.), None)), Some([100, 0]));
        assert_eq!(
            target(&c, &temperatures(Some(50.), Some(91.))),
            Some([0, 100])
        );
    }
    #[test]
    fn missing_cpu_restores_firmware() {
        assert_eq!(target(&custom(), &temperatures(None, Some(60.))), None);
    }
    #[test]
    fn first_and_recovery_samples_use_exact_target() {
        let c = custom();
        assert_eq!(
            target(&c, &temperatures(Some(50.), Some(50.))),
            Some([30, 30])
        );
        assert_eq!(target(&c, &temperatures(Some(50.), None)), Some([30, 0]));
        assert_eq!(
            target(&c, &temperatures(Some(50.), Some(50.))),
            Some([30, 30])
        );
    }
    #[test]
    fn legacy_shared_curve_migrates_to_both_channels() {
        let json = r#"{"mode":"Curve","manual":[35,0],"curve":[[40,0],[85,100]]}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.fans[0].curve, c.fans[1].curve);
        assert_eq!(c.fans[1].manual, 0);
        assert!(c.validate().is_ok());
    }
    #[test]
    fn legacy_manual_migrates() {
        let c: Config =
            serde_json::from_str(r#"{"mode":"Manual","manual":[35,0],"curve":[[40,0],[85,100]]}"#)
                .unwrap();
        assert_eq!(target(&c, &temperatures(Some(50.), None)), Some([35, 0]));
        let roundtrip: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, roundtrip);
    }
    #[test]
    fn hwmon_uses_hottest_cpu() {
        let p = std::env::temp_dir().join(format!("raf-test-{}", std::process::id()));
        let d = p.join("hwmon0");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("name"), "coretemp").unwrap();
        fs::write(d.join("temp1_input"), "45000").unwrap();
        fs::write(d.join("temp2_input"), "71000").unwrap();
        assert_eq!(read_temperatures(&p).cpu, Some(71.));
        fs::remove_dir_all(p).unwrap();
    }
}
