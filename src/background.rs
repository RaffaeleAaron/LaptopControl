use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
pub fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })))
        .join("laptop-control")
}
pub fn helper() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()?
                .parent()
                .map(|p| p.join("share/laptop-control/camera.py"))
        })
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|p| p.join("laptop-control/camera.py"))
                .filter(|p| p.is_file())
        })
        .or_else(|| {
            let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helpers/camera.py");
            p.is_file().then_some(p)
        })
        .unwrap_or_else(|| PathBuf::from("/usr/local/share/laptop-control/camera.py"))
}
pub fn connect() -> Result<UnixStream, String> {
    let socket = runtime().join("camera.sock");
    if let Ok(stream) = UnixStream::connect(&socket) {
        return Ok(stream);
    }
    let started = Command::new("/usr/bin/systemctl")
        .args(["--user", "start", "laptop-control.service"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !started {
        let gui = std::env::current_exe().map_err(|e| e.to_string())?;
        Command::new("/usr/bin/python3")
            .arg(helper())
            .args(["--service", "--gui"])
            .arg(gui)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    for _ in 0..40 {
        if let Ok(stream) = UnixStream::connect(&socket) {
            return Ok(stream);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("The Laptop Control background service could not start".into())
}
pub fn show_existing() -> bool {
    let Ok(mut stream) = connect() else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = serde_json::json!({"op":"show_existing", "token":std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default()});
    if writeln!(stream, "{request}").is_err() {
        return false;
    }
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|v| v["shown"].as_bool())
        .unwrap_or(false)
}
pub fn gui_lock() -> Option<fs::File> {
    let dir = runtime();
    fs::create_dir_all(&dir).ok()?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("gui.lock"))
        .ok()?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Some(file)
    } else {
        None
    }
}
