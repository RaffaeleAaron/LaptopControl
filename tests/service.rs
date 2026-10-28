use raf_fans::*;
use std::{
    io::Write,
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Service {
    child: Child,
    socket: String,
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(format!("{}.lock", self.socket));
    }
}
fn start(delay: u64) -> Service {
    start_with_temperatures(delay, None)
}
fn start_with_temperatures(delay: u64, temperatures: Option<&str>) -> Service {
    start_with_options(delay, temperatures, None)
}
fn start_with_options(delay: u64, temperatures: Option<&str>, state_dir: Option<&str>) -> Service {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let socket = format!(
        "/tmp/raf-integration-{}-{sequence}.sock",
        std::process::id()
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_raf-fand"));
    command.args([
        "--simulate",
        "--socket",
        &socket,
        "--simulate-delay-ms",
        &delay.to_string(),
    ]);
    if let Some(path) = temperatures {
        command.args(["--simulate-temperatures", path]);
    }
    if let Some(path) = state_dir {
        command.args(["--simulate-state-dir", path]);
    }
    let child = command.stdout(Stdio::null()).spawn().unwrap();
    let service = Service { child, socket };
    let deadline = Instant::now() + Duration::from_secs(5);
    while exchange(&service.socket, Request::Status).is_err() {
        assert!(Instant::now() < deadline, "Service startup timed out");
        thread::sleep(Duration::from_millis(25));
    }
    service
}
fn applied(service: &Service, config: Config) -> Status {
    exchange(&service.socket, Request::Apply(config)).unwrap();
    thread::sleep(Duration::from_millis(150));
    exchange(&service.socket, Request::Status).unwrap()
}
#[test]
fn simulated_service_validates_controls_and_restores() {
    let mut service = start(0);
    let s = exchange(&service.socket, Request::Status).unwrap();
    assert!(s.simulated);
    assert_eq!(s.config.mode, Mode::Firmware);
    assert_eq!(s.requested, None);
    let mut config = Config {
        mode: Mode::Curve,
        ..Config::default()
    };
    config.fans[0].mode = Mode::Manual;
    config.fans[0].manual = 70;
    config.fans[1].mode = Mode::Manual;
    config.fans[1].manual = 80;
    let s = applied(&service, config.clone());
    assert_eq!(s.requested, Some([70, 80]));
    config.fans[1].manual = 101;
    assert!(exchange(&service.socket, Request::Apply(config)).is_err());
    let mut stream = UnixStream::connect(&service.socket).unwrap();
    stream.write_all(b"not-json\n").unwrap();
    drop(stream);
    let s = applied(&service, Config::default());
    assert_eq!(s.requested, None);
    assert_eq!(s.config.mode, Mode::Firmware);
    // A client that never sends a newline must not block temperature control.
    let _slow = UnixStream::connect(&service.socket).unwrap();
    thread::sleep(Duration::from_millis(150));
    assert!(exchange(&service.socket, Request::Status).is_ok());
    let status = Command::new("kill")
        .args(["-TERM", &service.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = service.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!std::path::Path::new(&service.socket).exists());
}

#[test]
fn hardware_delay_does_not_block_status_or_apply() {
    let service = start(3000);
    thread::sleep(Duration::from_millis(200));
    let begin = Instant::now();
    let status = exchange(&service.socket, Request::Status).unwrap();
    assert!(begin.elapsed() < Duration::from_millis(500));
    assert!(status.sample_age_ms >= 200);
    let begin = Instant::now();
    exchange(&service.socket, Request::Apply(Config::default())).unwrap();
    assert!(begin.elapsed() < Duration::from_millis(500));
}

#[test]
fn gpu_power_transitions_keep_cpu_curve_and_return_directly_to_targets() {
    let path = std::env::temp_dir().join(format!("raf-transition-{}.json", std::process::id()));
    let sample = |gpu: Option<f32>, state: &str| {
        let t = Temperatures {
            cpu: Some(50.),
            gpu,
            gpu_expected: true,
            gpu_state: state.into(),
            ..Temperatures::default()
        };
        let next = path.with_extension("new");
        std::fs::write(&next, serde_json::to_vec(&t).unwrap()).unwrap();
        std::fs::rename(next, &path).unwrap();
    };
    sample(Some(50.), "D0");
    let service = start_with_temperatures(0, Some(path.to_str().unwrap()));
    let config = Config {
        mode: Mode::Curve,
        ..Config::default()
    };
    exchange(&service.socket, Request::Apply(config)).unwrap();
    let wait_for = |expected: [u8; 2], state: &str, recovering: bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = exchange(&service.socket, Request::Status).unwrap();
            if recovering {
                assert!(
                    s.requested.is_some(),
                    "GPU state change must not enter firmware mode"
                );
                assert_eq!(s.requested.unwrap()[0], 30);
                assert!(
                    s.requested.unwrap()[1] == 0 || (25..=30).contains(&s.requested.unwrap()[1])
                );
            }
            if s.requested == Some(expected) && s.temperatures.gpu_state == state {
                assert_eq!(s.config.mode, Mode::Curve);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Expected {expected:?}, got {:?}",
                s.requested
            );
            thread::sleep(Duration::from_millis(25));
        }
    };
    wait_for([30, 30], "D0", false);
    for state in ["Unknown", "D3hot", "D3cold", "D0"] {
        sample(None, state);
        wait_for([30, 0], state, true);
    }
    sample(Some(50.), "D0");
    wait_for([30, 30], "D0", true);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn selected_profile_survives_service_restart() {
    let directory = std::env::temp_dir().join(format!("laptop-fan-restart-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let service = start_with_options(0, None, Some(directory.to_str().unwrap()));
    let mut office = Config {
        mode: Mode::Curve,
        ..Config::default()
    };
    for fan in &mut office.fans {
        fan.mode = Mode::Manual;
    }
    office.fans[0].manual = 37;
    office.fans[0].sensor = Sensor::NvidiaSmi;
    office.fans[1].sensor = Sensor::Cpu;
    office.fans[1].manual = 0;
    let s = exchange(
        &service.socket,
        Request::SaveProfile {
            profile: Profile::Office,
            config: office.clone(),
        },
    )
    .unwrap();
    assert_eq!(s.profiles.office, office);
    let mut gaming = office.clone();
    gaming.fans[0].manual = 55;
    exchange(
        &service.socket,
        Request::SaveProfile {
            profile: Profile::Gaming,
            config: gaming.clone(),
        },
    )
    .unwrap();
    let s = exchange(&service.socket, Request::SelectProfile(Profile::Office)).unwrap();
    assert_eq!(s.config, office);
    assert_eq!(s.profiles.gaming, gaming);
    let settings = Settings {
        gpu_poll_seconds: 17,
        ..Settings::default()
    };
    let s = exchange(&service.socket, Request::SaveSettings(settings.clone())).unwrap();
    assert_eq!(s.profiles.settings, settings);
    assert_eq!(s.profiles.selected, Profile::Office);
    for invalid in [0, 301, u64::MAX] {
        assert!(exchange(
            &service.socket,
            Request::SaveSettings(Settings {
                gpu_poll_seconds: invalid,
                ..Settings::default()
            })
        )
        .is_err());
    }
    drop(service);
    let service = start_with_options(0, None, Some(directory.to_str().unwrap()));
    let s = exchange(&service.socket, Request::Status).unwrap();
    assert_eq!(s.profiles.selected, Profile::Office);
    assert_eq!(s.config, office);
    assert_eq!(s.profiles.gaming, gaming);
    assert_eq!(s.profiles.settings, settings);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = exchange(&service.socket, Request::Status).unwrap();
        if s.requested == Some([37, 0]) {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(25));
    }
    exchange(&service.socket, Request::SelectProfile(Profile::Firmware)).unwrap();
    drop(service);
    let service = start_with_options(0, None, Some(directory.to_str().unwrap()));
    assert_eq!(
        exchange(&service.socket, Request::Status)
            .unwrap()
            .profiles
            .selected,
        Profile::Firmware
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn smoothing_delivers_four_steps_then_bypasses_for_heat() {
    let path = std::env::temp_dir().join(format!("raf-smoothing-{}.json", std::process::id()));
    let sample = |cpu| {
        let t = Temperatures {
            cpu: Some(cpu),
            ..Temperatures::default()
        };
        let next = path.with_extension("new");
        std::fs::write(&next, serde_json::to_vec(&t).unwrap()).unwrap();
        std::fs::rename(next, &path).unwrap();
    };
    sample(50.);
    let service = start_with_temperatures(0, Some(path.to_str().unwrap()));
    let mut c = Config {
        mode: Mode::Curve,
        ..Config::default()
    };
    c.fans[1].mode = Mode::Manual;
    c.fans[1].manual = 0;
    let s = applied(&service, c);
    assert_eq!(s.requested, Some([30, 0]));
    sample(75.);
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut seen = vec![30];
    let mut steps = Vec::new();
    loop {
        let status = exchange(&service.socket, Request::Status).unwrap();
        let speed = status.requested.unwrap()[0];
        if seen.last() != Some(&speed) {
            seen.push(speed);
            steps.push(Instant::now());
        }
        if speed == 70 {
            break;
        }
        assert!(Instant::now() < deadline, "Ramp did not finish: {seen:?}");
        thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(seen, vec![30, 40, 50, 60, 70]);
    let elapsed = steps.last().unwrap().duration_since(steps[0]);
    assert!((Duration::from_millis(1200)..Duration::from_millis(2300)).contains(&elapsed));
    sample(91.);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let status = exchange(&service.socket, Request::Status).unwrap();
        if status.temperatures.cpu == Some(91.) {
            assert_eq!(status.requested, Some([100, 0]));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(15));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn averaging_softens_a_brief_spike_but_never_delays_emergency_or_sensor_loss() {
    let path = std::env::temp_dir().join(format!("raf-averaging-{}.json", std::process::id()));
    let sample = |cpu| {
        let t = Temperatures {
            cpu,
            ..Temperatures::default()
        };
        let next = path.with_extension("new");
        std::fs::write(&next, serde_json::to_vec(&t).unwrap()).unwrap();
        std::fs::rename(next, &path).unwrap();
    };
    sample(Some(50.));
    let service = start_with_temperatures(0, Some(path.to_str().unwrap()));
    let mut c = Config {
        mode: Mode::Curve,
        ..Config::default()
    };
    c.fans[0].average_seconds = 5;
    c.fans[0].smoothing.duration_ms = 0;
    c.fans[1].mode = Mode::Manual;
    c.fans[1].manual = 0;
    assert_eq!(applied(&service, c.clone()).requested, Some([30, 0]));
    thread::sleep(Duration::from_millis(3200));
    let await_sample = |expected| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status = exchange(&service.socket, Request::Status).unwrap();
            if status.temperatures.cpu == expected {
                break status;
            }
            assert!(Instant::now() < deadline, "Averaging sample timed out");
            thread::sleep(Duration::from_millis(15));
        }
    };
    sample(Some(75.));
    let s = await_sample(Some(75.));
    assert!(
        s.control_temperatures[0].unwrap() < 60.,
        "{:?}",
        s.control_temperatures
    );
    assert!(s.requested.unwrap()[0] < 70);
    sample(Some(50.));
    await_sample(Some(50.));
    sample(Some(91.));
    assert_eq!(await_sample(Some(91.)).requested, Some([100, 0]));
    sample(None);
    assert_eq!(await_sample(None).requested, None);
    sample(Some(50.));
    let s = await_sample(Some(50.));
    assert_eq!(s.control_temperatures[0], Some(50.));
    assert_eq!(s.requested, Some([30, 0]));
    c.fans[0].average_seconds = 61;
    assert!(exchange(&service.socket, Request::Apply(c)).is_err());
    std::fs::remove_file(path).unwrap();
}
