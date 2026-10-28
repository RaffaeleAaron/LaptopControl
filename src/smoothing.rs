use crate::{hardware::effective_percent, Config, Mode, Temperatures};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Smoothing {
    pub duration_ms: u64,
    pub steps: u8,
}
impl Default for Smoothing {
    fn default() -> Self {
        Self {
            duration_ms: 2000,
            steps: 4,
        }
    }
}
impl Smoothing {
    pub fn validate(&self) -> Result<(), String> {
        if self.duration_ms > 10000
            || !(1..=20).contains(&self.steps)
            || (self.duration_ms > 0 && self.duration_ms / u64::from(self.steps) < 100)
        {
            return Err("Use 0–10 seconds and 1–20 steps, at least 100 ms apart".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Default)]
struct Transition {
    from: u8,
    to: u8,
    start: u64,
    duration: u64,
    steps: u8,
}
#[derive(Default)]
pub struct Ramp {
    current: Option<[u8; 2]>,
    transitions: [Transition; 2],
}
impl Ramp {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn retarget(&mut self, config: &Config, t: &Temperatures, speeds: [u8; 2], now: u64) {
        let initial = self.current.is_none();
        let mut current = self.current.unwrap_or(speeds);
        for i in 0..2 {
            let f = &config.fans[i];
            let hot = (f.mode == Mode::Curve && f.sensor.temperature(t).is_some_and(|v| v >= 90.))
                || t.physical_temperature(i).is_some_and(|v| v >= 90.);
            let duration = if initial || hot || f.mode == Mode::Manual {
                0
            } else {
                f.smoothing.duration_ms
            };
            if self.transitions[i].to != speeds[i] || duration == 0 {
                self.transitions[i] = Transition {
                    from: current[i],
                    to: speeds[i],
                    start: now,
                    duration,
                    steps: f.smoothing.steps,
                };
                if duration == 0 {
                    current[i] = speeds[i];
                }
            }
        }
        self.current = Some(current);
    }
    pub fn advance(&mut self, now: u64) -> Option<[u8; 2]> {
        let mut current = self.current?;
        for (i, transition) in self.transitions.iter().enumerate() {
            let elapsed = now.saturating_sub(transition.start);
            let fraction = if transition.duration == 0 || elapsed >= transition.duration {
                1.
            } else {
                let step = elapsed * u64::from(transition.steps) / transition.duration;
                step as f32 / f32::from(transition.steps)
            };
            current[i] = effective_percent(
                (f32::from(transition.from)
                    + (f32::from(transition.to) - f32::from(transition.from)) * fraction)
                    .round() as u8,
            );
        }
        self.current = Some(current);
        Some(current)
    }
    pub fn next_ms(&self, now: u64) -> Option<u64> {
        self.transitions
            .iter()
            .filter_map(|t| {
                let elapsed = now.saturating_sub(t.start);
                if t.duration == 0 || elapsed >= t.duration {
                    None
                } else {
                    let step = elapsed * u64::from(t.steps) / t.duration + 1;
                    Some(t.start + (step * t.duration).div_ceil(u64::from(t.steps)))
                }
            })
            .min()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_steps_retarget_and_emergency_override() {
        let c = Config {
            mode: Mode::Curve,
            ..Config::default()
        };
        let mut t = Temperatures {
            cpu: Some(50.),
            gpu: Some(50.),
            ..Temperatures::default()
        };
        let mut r = Ramp::default();
        r.retarget(&c, &t, [40, 40], 0);
        assert_eq!(r.advance(0), Some([40, 40]));
        r.retarget(&c, &t, [80, 40], 100);
        for (ms, speed) in [(100, 40), (600, 50), (1100, 60), (1600, 70), (2100, 80)] {
            assert_eq!(r.advance(ms).unwrap()[0], speed);
        }
        r.retarget(&c, &t, [40, 40], 2200);
        r.advance(2700);
        r.retarget(&c, &t, [90, 40], 2750);
        assert_eq!(r.advance(3250).unwrap()[0], 75);
        t.cpu = Some(91.);
        r.retarget(&c, &t, [100, 40], 3300);
        assert_eq!(r.advance(3300), Some([100, 40]));
        assert!(Smoothing {
            duration_ms: 100,
            steps: 20
        }
        .validate()
        .is_err());
    }
}
