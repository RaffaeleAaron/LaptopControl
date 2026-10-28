use crate::{read_temperatures, Config, Settings, Temperatures};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn read(path: &Path, name: &str) -> String {
    fs::read_to_string(path.join(name))
        .unwrap_or_default()
        .trim()
        .to_string()
}
/// Both checks must positively agree. Unknown, suspended and transitional states deny access.
pub fn is_d0(path: &Path) -> bool {
    read(path, "power_state") == "D0" && read(path, "power/runtime_status") == "active"
}
fn temp_file(path: &Path) -> Option<f32> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<f32>()
        .ok()
        .map(|v| v / 1000.)
        .filter(|v| v.is_finite() && (1. ..=125.).contains(v))
}
fn sample_with(
    pci_root: &Path,
    hwmon_root: &Path,
    mut smi: impl FnMut(&Path) -> Option<NvidiaReading>,
) -> Temperatures {
    // NVIDIA hwmon temperatures are deliberately excluded by read_temperatures.
    let mut t = read_temperatures(hwmon_root);
    let mut states = Vec::new();
    let mut missing_awake = false;
    if let Ok(devices) = fs::read_dir(pci_root) {
        for device in devices.flatten() {
            let path = device.path();
            if read(&path, "vendor") != "0x10de" || !read(&path, "class").starts_with("0x03") {
                continue;
            }
            let state = read(&path, "power_state");
            states.push(if state.is_empty() {
                "Unknown".into()
            } else {
                state.clone()
            });
            if matches!(state.as_str(), "D3hot" | "D3cold") {
                continue;
            }
            if !is_d0(&path) {
                t.gpu_expected = true;
                continue;
            }
            let mut temp = None;
            if let Ok(monitors) = fs::read_dir(path.join("hwmon")) {
                for monitor in monitors.flatten() {
                    // Recheck before each GPU hwmon sensor access.
                    if !is_d0(&path) {
                        break;
                    }
                    if let Some(v) = temp_file(&monitor.path().join("temp1_input")) {
                        temp = Some(temp.unwrap_or(v).max(v));
                    }
                }
            }
            // SMI is a separate, explicitly selected source, never a hwmon fallback.
            if is_d0(&path) {
                if let Some(v) = smi(&path) {
                    if let Some(temp) = v.temperature {
                        t.gpu_smi = Some(t.gpu_smi.unwrap_or(temp).max(temp));
                    }
                    if let Some(usage) = v.usage {
                        t.gpu_usage = Some(t.gpu_usage.unwrap_or(usage).max(usage));
                    }
                }
            }
            // Never retain measurements from a device that went to sleep during sampling.
            if !is_d0(&path) {
                if !matches!(read(&path, "power_state").as_str(), "D3hot" | "D3cold") {
                    t.gpu_expected = true;
                }
                t.gpu_smi = None;
                t.gpu_usage = None;
                continue;
            }
            t.gpu_expected = true;
            if let Some(v) = temp {
                t.gpu = Some(t.gpu.unwrap_or(v).max(v));
            } else {
                missing_awake = true;
            }
        }
    }
    if missing_awake {
        t.gpu = None;
    }
    t.gpu_state = if states.is_empty() {
        "Not detected".into()
    } else {
        states.join(" / ")
    };
    t
}
pub fn sample_at(pci_root: &Path, hwmon_root: &Path) -> Temperatures {
    sample_with(pci_root, hwmon_root, |_| None)
}
/// Read-only hwmon sampling, including when probing hardware outside the running service.
pub fn sample() -> Temperatures {
    sample_at(
        Path::new("/sys/bus/pci/devices"),
        Path::new("/sys/class/hwmon"),
    )
}
// NVIDIA CLI calls run away from the fan-control loop and IPC. A slow or broken
// driver cannot hold up CPU sampling, target changes or profile switches.
#[derive(Clone, Copy)]
struct NvidiaReading {
    temperature: Option<f32>,
    usage: Option<f32>,
}
struct Query {
    path: PathBuf,
    id: u64,
    epoch: u64,
}
struct QueryResult {
    path: PathBuf,
    id: u64,
    temp: Option<NvidiaReading>,
    sampled: Instant,
}
struct Reading {
    attempted: Instant,
    pending: Option<u64>,
    value: Option<(NvidiaReading, Instant)>,
}
pub struct Sampler {
    tx: mpsc::Sender<Query>,
    rx: mpsc::Receiver<QueryResult>,
    epoch: Arc<AtomicU64>,
    enabled: bool,
    interval: Duration,
    next_id: u64,
    readings: HashMap<PathBuf, Reading>,
}
impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}
impl Sampler {
    pub fn new() -> Self {
        Self::with_reading(nvidia_reading)
    }
    #[cfg(test)]
    fn with_query(query: impl Fn(&Path) -> Option<f32> + Send + 'static) -> Self {
        Self::with_reading(move |p| {
            query(p).map(|t| NvidiaReading {
                temperature: Some(t),
                usage: None,
            })
        })
    }
    fn with_reading(query: impl Fn(&Path) -> Option<NvidiaReading> + Send + 'static) -> Self {
        let (tx, commands) = mpsc::channel::<Query>();
        let (results, rx) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        thread::spawn(move || {
            while let Ok(request) = commands.recv() {
                // Cancel queued work after deselection or a settings change.
                if request.epoch != worker_epoch.load(Ordering::Acquire) {
                    continue;
                }
                let temp = if is_d0(&request.path) {
                    query(&request.path)
                } else {
                    None
                };
                if results
                    .send(QueryResult {
                        path: request.path,
                        id: request.id,
                        temp,
                        sampled: Instant::now(),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            tx,
            rx,
            epoch,
            enabled: false,
            interval: Duration::ZERO,
            next_id: 0,
            readings: HashMap::new(),
        }
    }
    pub fn sample(&mut self, config: &Config, settings: &Settings) -> Temperatures {
        self.sample_at(
            Path::new("/sys/bus/pci/devices"),
            Path::new("/sys/class/hwmon"),
            config,
            settings,
        )
    }
    fn sample_at(
        &mut self,
        pci: &Path,
        hwmon: &Path,
        config: &Config,
        settings: &Settings,
    ) -> Temperatures {
        let enabled = config.uses_nvidia_smi()
            || (config.mode != crate::Mode::Firmware && settings.gpu_usage_in_gaming);
        let interval = Duration::from_secs(settings.gpu_poll_seconds.clamp(1, 300));
        if enabled != self.enabled || interval != self.interval {
            self.epoch.fetch_add(1, Ordering::AcqRel);
            self.readings.clear();
            self.enabled = enabled;
            self.interval = interval;
        }
        while let Ok(result) = self.rx.try_recv() {
            if let Some(reading) = self.readings.get_mut(&result.path) {
                if reading.pending == Some(result.id) {
                    reading.pending = None;
                    reading.value = result.temp.map(|t| (t, result.sampled));
                }
            }
        }
        let mut awake = HashSet::new();
        let t = sample_with(pci, hwmon, |path| {
            if !enabled {
                return None;
            }
            awake.insert(path.to_path_buf());
            let now = Instant::now();
            let reading = self.readings.entry(path.to_path_buf()).or_insert(Reading {
                attempted: now.checked_sub(interval).unwrap_or(now),
                pending: None,
                value: None,
            });
            if reading.pending.is_none() && reading.attempted.elapsed() >= interval {
                self.next_id += 1;
                let request = Query {
                    path: path.to_path_buf(),
                    id: self.next_id,
                    epoch: self.epoch.load(Ordering::Acquire),
                };
                if self.tx.send(request).is_ok() {
                    reading.pending = Some(self.next_id);
                    reading.attempted = now;
                }
            }
            // Retain the last completed reading between scheduled polls, never
            // indefinitely on failures or across sleep / source deselection.
            reading
                .value
                .filter(|(_, sampled)| sampled.elapsed() <= interval + Duration::from_secs(2))
                .map(|(t, _)| t)
        });
        self.readings.retain(|path, _| awake.contains(path));
        t
    }
}
fn nvidia_reading(path: &Path) -> Option<NvidiaReading> {
    // Check immediately before starting the driver query as well as in sampling.
    if !is_d0(path) {
        return None;
    }
    let bus_id = path.file_name()?.to_str()?;
    let mut child = Command::new("/usr/bin/nvidia-smi")
        .args([
            "--id",
            bus_id,
            "--query-gpu=temperature.gpu,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() || !is_d0(path) {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let mut values = text.trim().split(',');
    let temperature = values
        .next()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite() && (1. ..=125.).contains(v));
    let usage = values
        .next()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite() && (0. ..=100.).contains(v));
    Some(NvidiaReading { temperature, usage })
}
/// Reassert the current targets after AC changes, without returning to firmware mode.
pub fn ac_state() -> String {
    let mut sources = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for entry in entries.flatten() {
            if read(&entry.path(), "type") == "Mains" {
                sources.push(format!(
                    "{}:{}",
                    entry.file_name().to_string_lossy(),
                    read(&entry.path(), "online")
                ));
            }
        }
    }
    sources.sort();
    sources.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        gpu: PathBuf,
    }
    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("raf-poll-{name}-{}", std::process::id()));
            let gpu = root.join("pci/0000:01:00.0");
            fs::create_dir_all(gpu.join("power")).unwrap();
            fs::write(gpu.join("vendor"), "0x10de").unwrap();
            fs::write(gpu.join("class"), "0x030000").unwrap();
            fs::create_dir_all(root.join("hwmon/cpu")).unwrap();
            fs::write(root.join("hwmon/cpu/name"), "coretemp").unwrap();
            fs::write(root.join("hwmon/cpu/temp1_input"), "50000").unwrap();
            let f = Self { root, gpu };
            f.state("D0", "active");
            f
        }
        fn state(&self, state: &str, runtime: &str) {
            fs::write(self.gpu.join("power_state"), state).unwrap();
            fs::write(self.gpu.join("power/runtime_status"), runtime).unwrap();
        }
        fn sample(&self, s: &mut Sampler, c: &Config) -> Temperatures {
            s.sample_at(
                &self.root.join("pci"),
                &self.root.join("hwmon"),
                c,
                &Settings::default(),
            )
        }
        fn wait_for(&self, s: &mut Sampler, c: &Config, expected: Option<f32>) {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let t = self.sample(s, c);
                if t.gpu_smi == expected {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Expected {expected:?}, got {:?}",
                    t.gpu_smi
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn smi_config() -> Config {
        let mut c = Config {
            mode: crate::Mode::Curve,
            ..Config::default()
        };
        for fan in &mut c.fans {
            fan.sensor = crate::Sensor::NvidiaSmi;
        }
        c
    }
    #[test]
    fn polling_is_opt_in_shared_throttled_and_cleared_on_sleep() {
        use std::sync::atomic::AtomicUsize;
        let f = Fixture::new("schedule");
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut s = Sampler::with_query(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Some(70.)
        });
        let mut c = Config {
            mode: crate::Mode::Curve,
            ..Config::default()
        };
        assert_eq!(f.sample(&mut s, &c).gpu_smi, None);
        assert!(s.readings.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        c = smi_config();
        for (state, runtime) in [
            ("D3hot", "suspended"),
            ("D3cold", "suspended"),
            ("Unknown", "active"),
            ("D0", "suspending"),
            ("D0", "suspended"),
        ] {
            f.state(state, runtime);
            assert_eq!(f.sample(&mut s, &c).gpu_smi, None);
            assert!(s.readings.is_empty());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        f.state("D0", "active");
        f.wait_for(&mut s, &c, Some(70.));
        for _ in 0..5 {
            assert_eq!(f.sample(&mut s, &c).gpu_smi, Some(70.));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Both fans must share a single query"
        );
        s.readings.get_mut(&f.gpu).unwrap().attempted -= Duration::from_secs(5);
        f.sample(&mut s, &c);
        let deadline = Instant::now() + Duration::from_secs(1);
        while calls.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        f.state("D3cold", "suspended");
        assert_eq!(f.sample(&mut s, &c).gpu_smi, None);
        assert!(s.readings.is_empty());
        f.state("D0", "active");
        c.fans[0].sensor = crate::Sensor::Cpu;
        c.fans[1].sensor = crate::Sensor::Gpu;
        assert_eq!(f.sample(&mut s, &c).gpu_smi, None);
        assert!(s.readings.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn slow_query_does_not_block_cpu_and_late_results_after_disable_are_ignored() {
        let f = Fixture::new("slow");
        let (entered, rx) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut s = Sampler::with_query(move |_| {
            entered.send(()).unwrap();
            blocked.recv().unwrap();
            Some(80.)
        });
        let c = smi_config();
        let begin = Instant::now();
        let t = f.sample(&mut s, &c);
        assert_eq!(t.cpu, Some(50.));
        assert_eq!(t.gpu_smi, None);
        assert!(begin.elapsed() < Duration::from_millis(100));
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let off = Config::default();
        assert_eq!(f.sample(&mut s, &off).gpu_smi, None);
        release.send(()).unwrap();
        thread::sleep(Duration::from_millis(20));
        assert_eq!(f.sample(&mut s, &off).gpu_smi, None);
        assert!(s.readings.is_empty());
    }
    #[test]
    fn interval_changes_reschedule_and_query_failure_clears_cached_temperature() {
        use std::sync::atomic::AtomicUsize;
        let f = Fixture::new("failure");
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut s = Sampler::with_query(move |_| {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Some(70.)
            } else {
                None
            }
        });
        let c = smi_config();
        f.wait_for(&mut s, &c, Some(70.));
        let settings = Settings {
            gpu_poll_seconds: 30,
            ..Settings::default()
        };
        let t = s.sample_at(&f.root.join("pci"), &f.root.join("hwmon"), &c, &settings);
        assert_eq!(t.gpu_smi, None);
        assert_eq!(s.interval, Duration::from_secs(30));
        let deadline = Instant::now() + Duration::from_secs(1);
        while s.readings[&f.gpu].pending.is_some() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
            s.sample_at(&f.root.join("pci"), &f.root.join("hwmon"), &c, &settings);
        }
        assert!(s.readings[&f.gpu].value.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn hwmon_only_and_d0_gated() {
        let root = std::env::temp_dir().join(format!("raf-gpu-{}", std::process::id()));
        let gpu = root.join("pci/0000:01:00.0");
        fs::create_dir_all(gpu.join("power")).unwrap();
        fs::write(gpu.join("vendor"), "0x10de").unwrap();
        fs::write(gpu.join("class"), "0x030000").unwrap();
        let hwmon = gpu.join("hwmon/hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(hwmon.join("temp1_input"), "50000").unwrap();
        for (state, runtime) in [
            ("D3hot", "suspended"),
            ("D3cold", "suspended"),
            ("unknown", "active"),
            ("D0", "suspending"),
            ("D0", "suspended"),
        ] {
            fs::write(gpu.join("power_state"), state).unwrap();
            fs::write(gpu.join("power/runtime_status"), runtime).unwrap();
            let t = sample_at(&root.join("pci"), &root.join("empty"));
            assert_eq!(t.gpu, None);
            if state.starts_with("D3") {
                assert!(!t.gpu_expected);
            }
        }
        fs::write(gpu.join("power_state"), "D0").unwrap();
        fs::write(gpu.join("power/runtime_status"), "active").unwrap();
        let t = sample_at(&root.join("pci"), &root.join("empty"));
        assert_eq!(t.gpu, Some(50.));
        assert!(t.gpu_expected);
        fs::remove_file(hwmon.join("temp1_input")).unwrap();
        // An awake GPU without hwmon stays unavailable: no external CLI fallback exists.
        let t = sample_at(&root.join("pci"), &root.join("empty"));
        assert_eq!(t.gpu, None);
        assert!(t.gpu_expected);
        fs::remove_dir_all(root).unwrap();
    }
}
