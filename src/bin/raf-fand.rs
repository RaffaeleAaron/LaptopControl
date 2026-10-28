use raf_fans::{
    hardware::{effective_percent, Hardware},
    *,
};
use std::{
    fs,
    io::{self, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::{
            fs::PermissionsExt,
            net::{SocketAddr, UnixDatagram, UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}
fn notify(message: &str) {
    if let Ok(path) = std::env::var("NOTIFY_SOCKET") {
        if let Ok(sock) = UnixDatagram::unbound() {
            let addr = if let Some(name) = path.strip_prefix('@') {
                SocketAddr::from_abstract_name(name.as_bytes())
            } else {
                SocketAddr::from_pathname(path)
            };
            if let Ok(addr) = addr {
                let _ = sock.send_to_addr(message.as_bytes(), &addr);
            }
        }
    }
}
struct Shared {
    status: Status,
    revision: u64,
    sampled: Instant,
}
fn serve(
    mut stream: UnixStream,
    shared: &Arc<Mutex<Shared>>,
    state_dir: Option<&Path>,
    wake: &Condvar,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let error = match read_request(&stream) {
        Ok(Request::Status) => None,
        Ok(request) => {
            let mut profiles = shared.lock().unwrap().status.profiles.clone();
            let result = match request {
                Request::SelectProfile(profile) => {
                    profiles.selected = profile;
                    Ok(())
                }
                Request::SaveProfile { profile, config } => profiles.save_profile(profile, config),
                Request::SaveSettings(settings) => {
                    profiles.settings = settings;
                    Ok(())
                }
                Request::Apply(config) => {
                    if config.mode == Mode::Firmware {
                        profiles.selected = Profile::Firmware;
                        Ok(())
                    } else {
                        let profile = if profiles.selected == Profile::Firmware {
                            Profile::Office
                        } else {
                            profiles.selected
                        };
                        profiles.save_profile(profile, config)
                    }
                }
                Request::Status => unreachable!(),
            };
            match result.and_then(|()| profiles.validate()).and_then(|()| {
                if let Some(dir) = state_dir {
                    profiles
                        .persist(dir)
                        .map_err(|e| format!("Cannot save profiles: {e}"))?;
                }
                Ok(())
            }) {
                Err(e) => Some(e),
                Ok(()) => {
                    let mut s = shared.lock().unwrap();
                    s.status.config = profiles.active();
                    s.status.profiles = profiles;
                    s.revision += 1;
                    wake.notify_one();
                    s.status.message = "Saved configuration accepted; applying targets".into();
                    None
                }
            }
        }
        Err(e) => Some(e.to_string()),
    };
    let status = {
        let s = shared.lock().unwrap();
        let mut status = s.status.clone();
        status.sample_age_ms = s.sampled.elapsed().as_millis() as u64;
        status
    };
    serde_json::to_writer(&mut stream, &Reply { status, error })?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("raf-fand: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--restore"] {
        Hardware::open()?.restore()?;
        return Ok(());
    }
    if args == ["--probe"] {
        let h = Hardware::open()?;
        println!(
            "Uniwill detected; raw fan channels: {:?}; temperatures: {:?}",
            h.fans(),
            telemetry::sample()
        );
        return Ok(());
    }
    let simulated = args.iter().any(|a| a == "--simulate");
    let delay_ms = args
        .windows(2)
        .find(|w| w[0] == "--simulate-delay-ms")
        .and_then(|w| w[1].parse::<u64>().ok())
        .unwrap_or(0)
        .min(5000);
    let simulated_temperatures = args
        .windows(2)
        .find(|w| w[0] == "--simulate-temperatures")
        .map(|w| w[1].clone());
    let socket = args
        .windows(2)
        .find(|w| w[0] == "--socket")
        .map(|w| w[1].as_str())
        .unwrap_or(SOCKET);
    if !simulated && socket != SOCKET {
        return Err("Custom socket paths are only allowed in simulation".into());
    }
    if !simulated {
        for service in ["tccd.service", "fancontrol.service"] {
            if Command::new("/usr/bin/systemctl")
                .args(["is-active", "--quiet", service])
                .status()?
                .success()
            {
                return Err(format!("Stop {service} before starting raf-fand").into());
            }
        }
    }
    let mut hardware = if simulated {
        None
    } else {
        Some(Hardware::open()?)
    };
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{socket}.lock"))?;
    // SAFETY: valid fd, integer flags; flock prevents duplicate hardware writers.
    if unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&lock),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    } != 0
    {
        return Err("Another raf-fand is running".into());
    }
    if let Some(h) = &mut hardware {
        h.restore()?;
    }
    if Path::new(socket).exists() {
        fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;

    // SAFETY: signal handler only sets an atomic flag.
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    let state_dir = if simulated {
        args.windows(2)
            .find(|w| w[0] == "--simulate-state-dir")
            .map(|w| PathBuf::from(&w[1]))
    } else {
        Some(PathBuf::from("/var/lib/raf-fans"))
    };
    let profiles = if let Some(dir) = &state_dir {
        match Profiles::load(dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}; using firmware until settings are saved");
                Profiles::default()
            }
        }
    } else {
        Profiles::default()
    };
    let status = Status {
        config: profiles.active(),
        profiles,
        simulated,
        ..Status::default()
    };
    let shared = Arc::new(Mutex::new(Shared {
        status,
        revision: 0,
        sampled: Instant::now(),
    }));
    // Hardware access may take seconds. Never hold the status lock during EC I/O.
    let ipc = shared.clone();
    let wake = Arc::new(Condvar::new());
    let ipc_wake = wake.clone();
    let server = thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if STOP.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(e) = serve(stream, &ipc, state_dir.as_deref(), &ipc_wake) {
                        eprintln!("IPC request: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("IPC listener failed: {e}");
                    STOP.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    notify("READY=1");
    let mut last_sent = None;
    let mut last_ac = String::new();
    let mut firmware = true;
    let mut last_write = Instant::now() - Duration::from_secs(60);
    let mut last_fan_read = last_write;
    let mut raw_fans = [None, None];
    let mut last_cycle = Instant::now() - Duration::from_secs(2);
    let mut revision = u64::MAX;
    let mut sampler = telemetry::Sampler::new();
    let mut temperatures = Temperatures::default();
    let mut sampled = Instant::now();
    let mut ramp = smoothing::Ramp::default();
    let mut averages = averaging::Averages::default();
    let mut control_temperatures = [None; 2];
    let started = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        let (config, settings, current_revision) = {
            let s = shared.lock().unwrap();
            {
                let mut settings = s.status.profiles.settings.clone();
                if s.status.profiles.selected != Profile::Gaming {
                    settings.gpu_usage_in_gaming = false;
                }
                (s.status.config.clone(), settings, s.revision)
            }
        };
        let now = || started.elapsed().as_millis() as u64;
        let mut reassert = false;
        if last_cycle.elapsed() >= Duration::from_secs(1) || current_revision != revision {
            last_cycle = Instant::now();
            if current_revision != revision {
                averages.reset();
            }
            revision = current_revision;
            if simulated && delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
            temperatures = if simulated {
                if let Some(path) = &simulated_temperatures {
                    fs::read(path)
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default()
                } else {
                    Temperatures {
                        cpu: Some(62.),
                        gpu: Some(48.),
                        gpu_smi: config.uses_nvidia_smi().then_some(48.),
                        gpu_usage: settings.gpu_usage_in_gaming.then_some(24.),
                        gpu_expected: true,
                        gpu_state: "D0 (simulated)".into(),
                    }
                }
            } else {
                sampler.sample(&config, &settings)
            };
            sampled = Instant::now();
            if shared.lock().unwrap().revision != revision {
                continue;
            }
            let ac = if simulated {
                String::new()
            } else {
                telemetry::ac_state()
            };
            reassert = ac != last_ac;
            last_ac = ac;
            control_temperatures = averages.sample(&config, &temperatures, now());
            if let Some(speeds) = target_with_sources(&config, &temperatures, control_temperatures)
            {
                ramp.retarget(&config, &temperatures, speeds.map(effective_percent), now());
            } else {
                ramp.reset();
                averages.reset();
            }
        }
        let mut requested = None;
        let message = if let Some(speeds) = ramp.advance(now()) {
            if firmware
                || reassert
                || last_sent != Some(speeds)
                || last_write.elapsed() >= Duration::from_secs(10)
            {
                if let Some(h) = &mut hardware {
                    h.set_changed(
                        speeds,
                        if firmware || reassert || last_write.elapsed() >= Duration::from_secs(10) {
                            None
                        } else {
                            last_sent
                        },
                    )?;
                }
                last_write = Instant::now();
            }
            firmware = false;
            last_sent = Some(speeds);
            requested = Some(speeds);
            if temperatures.cpu.is_some_and(|t| t >= 90.)
                || temperatures.gpu.is_some_and(|t| t >= 90.)
                || temperatures.gpu_smi.is_some_and(|t| t >= 90.)
            {
                "Thermal override active on hot channel"
            } else if config
                .fans
                .iter()
                .any(|f| f.mode == Mode::Curve && f.sensor.temperature(&temperatures).is_none())
            {
                "Fan control active · unavailable sources use their configured fallback"
            } else {
                "Independent CPU and dGPU control active"
            }
        } else {
            if !firmware {
                if let Some(h) = &mut hardware {
                    h.restore()?;
                }
                firmware = true;
            }
            if temperatures.cpu.is_none() {
                "Sensor unavailable: firmware control restored"
            } else {
                "Firmware controls both fans"
            }
        };
        if last_fan_read.elapsed() >= Duration::from_secs(10) {
            if let Some(h) = &hardware {
                raw_fans = h.fans();
            }
            last_fan_read = Instant::now();
        }
        {
            let mut s = shared.lock().unwrap();
            s.status.temperatures = temperatures.clone();
            s.status.requested = requested;
            s.status.control_temperatures = control_temperatures;
            s.status.raw_fans = raw_fans;
            s.sampled = sampled;
            if s.revision == revision {
                s.status.message = message.into();
            }
        }
        notify("WATCHDOG=1");
        let mut delay = Duration::from_secs(1).saturating_sub(last_cycle.elapsed());
        if let Some(next) = ramp.next_ms(now()) {
            delay = delay.min(Duration::from_millis(next.saturating_sub(now())));
        }
        let guard = shared.lock().unwrap();
        if guard.revision == revision && !STOP.load(Ordering::Relaxed) {
            let _ = wake
                .wait_timeout(guard, delay.max(Duration::from_millis(5)))
                .unwrap();
        }
    }
    if let Some(h) = &mut hardware {
        h.restore()?;
    }
    let _ = UnixStream::connect(socket);
    let _ = server.join();
    fs::remove_file(socket)?;
    notify("STOPPING=1");
    Ok(())
}
