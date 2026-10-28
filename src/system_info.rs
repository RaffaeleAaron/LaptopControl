use crate::preferences::Cards;
use std::{
    fs,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct SystemInfo {
    pub model: String,
    pub cpu: String,
    pub memory: String,
    pub os: String,
    pub power: String,
    pub mode: String,
    pub cpu_usage: Option<f32>,
    previous_cpu: Option<(u64, u64)>,
    last: Option<Instant>,
}
fn text(p: &str) -> String {
    fs::read_to_string(p).unwrap_or_default().trim().to_owned()
}
fn number(p: &str) -> Option<f64> {
    text(p).parse().ok()
}
impl SystemInfo {
    pub fn new() -> Self {
        Self {
            model: text("/sys/class/dmi/id/product_name"),
            cpu: text("/proc/cpuinfo")
                .lines()
                .find_map(|l| {
                    l.strip_prefix("model name")
                        .and_then(|s| s.split_once(':'))
                        .map(|(_, s)| s.trim().to_owned())
                })
                .unwrap_or_default(),
            memory: text("/proc/meminfo")
                .lines()
                .find_map(|l| {
                    l.strip_prefix("MemTotal:")
                        .and_then(|s| s.split_whitespace().next())
                        .and_then(|s| s.parse::<f64>().ok())
                })
                .map(|n| format!("{:.1} GiB RAM", n / 1048576.))
                .unwrap_or_default(),
            os: text("/etc/os-release")
                .lines()
                .find_map(|l| {
                    l.strip_prefix("PRETTY_NAME=")
                        .map(|s| s.trim_matches('"').to_owned())
                })
                .unwrap_or_default(),
            ..Self::default()
        }
    }
    pub fn update(&mut self, cards: &Cards) {
        if (!cards.power && !cards.usage)
            || self
                .last
                .is_some_and(|i| i.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        self.last = Some(Instant::now());
        if cards.usage {
            let line = text("/proc/stat");
            let values: Vec<u64> = line
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .skip(1)
                .take(8)
                .filter_map(|s| s.parse().ok())
                .collect();
            if values.len() >= 5 {
                let total: u64 = values.iter().sum();
                let idle = values[3] + values[4];
                if let Some((old_total, old_idle)) = self.previous_cpu {
                    let delta = total.saturating_sub(old_total);
                    if delta > 0 {
                        self.cpu_usage =
                            Some(100. * (1. - idle.saturating_sub(old_idle) as f32 / delta as f32));
                    }
                }
                self.previous_cpu = Some((total, idle));
            }
        }
        if cards.power {
            self.mode = text("/sys/firmware/acpi/platform_profile");
            if self.mode.is_empty() {
                let epp =
                    text("/sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference");
                self.mode = if epp.is_empty() {
                    "Power mode unavailable".into()
                } else {
                    format!("CPU EPP: {epp}")
                };
            }
            self.power = "Battery power unavailable".into();
            if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if text(&p.join("type").to_string_lossy()) != "Battery" {
                        continue;
                    }
                    let watts = number(&p.join("power_now").to_string_lossy())
                        .map(|n| n / 1e6)
                        .or_else(|| {
                            Some(
                                number(&p.join("current_now").to_string_lossy())?
                                    * number(&p.join("voltage_now").to_string_lossy())?
                                    / 1e12,
                            )
                        });
                    let state = text(&p.join("status").to_string_lossy());
                    self.power = watts
                        .map(|w| format!("{w:.1} W · {state}"))
                        .unwrap_or(state);
                }
            }
        }
    }
}
