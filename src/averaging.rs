//! Bounded rolling temperature averages. Reuses the existing sensor cadence.
use crate::{Config, Mode, Sensor, Temperatures};
use std::collections::VecDeque;

#[derive(Default)]
struct Window {
    samples: VecDeque<(u64, f32)>,
    sum: f32,
    settings: Option<(Sensor, u16)>,
}
impl Window {
    fn sample(
        &mut self,
        sensor: Sensor,
        seconds: u16,
        value: Option<f32>,
        now: u64,
    ) -> Option<f32> {
        if self.settings != Some((sensor, seconds)) || value.is_none() {
            *self = Self::default();
            self.settings = Some((sensor, seconds));
        }
        let value = value?;
        if seconds == 0 {
            return Some(value);
        }
        // A settings update can cause another sample at the same instant.
        if self.samples.back().is_some_and(|s| s.0 == now) {
            self.sum -= self.samples.pop_back().unwrap().1;
        }
        self.samples.push_back((now, value));
        self.sum += value;
        while self
            .samples
            .front()
            .is_some_and(|s| now.saturating_sub(s.0) >= u64::from(seconds) * 1000)
        {
            self.sum -= self.samples.pop_front().unwrap().1;
        }
        Some(self.sum / self.samples.len() as f32)
    }
}

#[derive(Default)]
pub struct Averages {
    windows: [Window; 2],
}
impl Averages {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// Only the curve input is filtered. Callers retain raw physical readings for protection.
    pub fn sample(&mut self, config: &Config, raw: &Temperatures, now: u64) -> [Option<f32>; 2] {
        std::array::from_fn(|i| {
            let fan = &config.fans[i];
            if config.mode == Mode::Firmware || fan.mode != Mode::Curve {
                self.windows[i] = Window::default();
                return fan.sensor.temperature(raw);
            }
            self.windows[i].sample(
                fan.sensor,
                fan.average_seconds,
                fan.sensor.temperature(raw),
                now,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emergency_cross_source_and_missing_cpu_bypass_averaging() {
        let mut config = Config {
            mode: Mode::Curve,
            ..Config::default()
        };
        config.fans[0].average_seconds = 60;
        config.fans[0].sensor = Sensor::Gpu;
        let mut a = Averages::default();
        let mut raw = Temperatures {
            cpu: Some(50.),
            gpu: Some(30.),
            ..Temperatures::default()
        };
        a.sample(&config, &raw, 0);
        raw.gpu = Some(91.);
        let sources = a.sample(&config, &raw, 1000);
        assert!(sources[0].unwrap() < 90.);
        assert_eq!(
            crate::target_with_sources(&config, &raw, sources),
            Some([100, 100])
        );
        raw.gpu = None;
        let sources = a.sample(&config, &raw, 2000);
        assert_eq!(sources[0], None);
        assert_eq!(
            crate::target_with_sources(&config, &raw, sources),
            Some([0, 0])
        );
        raw.cpu = None;
        assert_eq!(crate::target_with_sources(&config, &raw, sources), None);
    }
    #[test]
    fn spike_is_averaged_then_expires_and_missing_sensor_resets_history() {
        let mut w = Window::default();
        for time in 0..5 {
            assert_eq!(w.sample(Sensor::Cpu, 5, Some(50.), time * 1000), Some(50.));
        }
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(75.), 5000), Some(55.));
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(50.), 6000), Some(55.));
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(50.), 10000), Some(50.));
        assert_eq!(w.sample(Sensor::Cpu, 5, None, 11000), None);
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(70.), 12000), Some(70.));
    }
    #[test]
    fn disabled_source_change_duplicate_samples_and_long_gaps() {
        let mut w = Window::default();
        assert_eq!(w.sample(Sensor::Cpu, 0, Some(75.), 0), Some(75.));
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(50.), 1000), Some(50.));
        assert_eq!(w.sample(Sensor::Cpu, 5, Some(70.), 1000), Some(70.));
        assert_eq!(w.samples.len(), 1);
        assert_eq!(w.sample(Sensor::Gpu, 5, Some(30.), 2000), Some(30.));
        assert_eq!(w.sample(Sensor::Gpu, 5, Some(80.), 20000), Some(80.));
        assert_eq!(w.samples.len(), 1);
    }
}
