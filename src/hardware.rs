use std::{
    fs::{self, File, OpenOptions},
    io,
    os::fd::AsRawFd,
};
// tuxedo-drivers 4.25.0 tuxedo_io_ioctl.h uses int32_t* in _IOR/_IOW.
// The encoded size is sizeof(pointer), but the payload is a four-byte integer.
const fn request(read: bool, magic: u64, nr: u64) -> libc::c_ulong {
    (((if read { 2 } else { 1 }) << 30)
        | ((std::mem::size_of::<*const i32>() as u64) << 16)
        | (magic << 8)
        | nr) as libc::c_ulong
}
pub const AUTO: libc::c_ulong = 0xf014;
pub struct Hardware {
    file: File,
    restore_on_drop: bool,
}
impl Hardware {
    pub fn open() -> io::Result<Self> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err(io::Error::other("Only Linux x86_64 is supported"));
        }
        let board = fs::read_to_string("/sys/class/dmi/id/board_name")?;
        let vendor = fs::read_to_string("/sys/class/dmi/id/sys_vendor")?;
        if vendor.trim() != "TUXEDO" || board.trim() != "PH6AG01_PH6AQ71_PH6AQI1" {
            return Err(io::Error::other(
                "This backend is restricted to the verified InfinityBook Pro Gen7 MK2 board",
            ));
        }
        let h = Self {
            file: OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tuxedo_io")?,
            restore_on_drop: false,
        };
        if h.read(request(true, 0xec, 6))? != 1 {
            return Err(io::Error::other("Uniwill interface not detected"));
        }
        Ok(h)
    }
    pub fn read(&self, command: libc::c_ulong) -> io::Result<i32> {
        let mut value: i32 = -1;
        // SAFETY: valid device fd and writable, aligned four-byte payload.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), command, &mut value) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if value < 0 {
            return Err(io::Error::other("Driver did not fill IOCTL payload"));
        }
        Ok(value)
    }
    pub fn fans(&self) -> [Option<i32>; 2] {
        [
            self.read(request(true, 0xef, 0x10)).ok(),
            self.read(request(true, 0xef, 0x11)).ok(),
        ]
    }
    pub fn set(&mut self, speeds: [u8; 2]) -> io::Result<()> {
        self.set_changed(speeds, None)
    }
    pub fn set_changed(&mut self, speeds: [u8; 2], previous: Option<[u8; 2]>) -> io::Result<()> {
        self.restore_on_drop = true;
        for (i, speed) in speeds.into_iter().enumerate() {
            if previous.is_some_and(|p| p[i] == speed) {
                continue;
            }
            if speed > 100 {
                return Err(io::Error::other("Speed outside safe range"));
            }
            let raw: i32 = i32::from(effective_percent(speed)) * 2;
            // SAFETY: the driver copies sizeof(i32) from this valid pointer.
            if unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    request(false, 0xf0, 0x10 + i as u64),
                    &raw,
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
    pub fn restore(&mut self) -> io::Result<()> {
        // SAFETY: W_UW_FANAUTO takes no payload and restores firmware control.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), AUTO) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.restore_on_drop = false;
        Ok(())
    }
}
impl Drop for Hardware {
    fn drop(&mut self) {
        if self.restore_on_drop {
            if let Err(e) = self.restore() {
                eprintln!("Firmware restoration failed: {e}");
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_matches_x86_64_header() {
        assert_eq!(request(true, 0xef, 0x10), 0x8008ef10);
        assert_eq!(request(false, 0xf0, 0x11), 0x4008f011);
        assert_eq!(request(true, 0xec, 6), 0x8008ec06);
    }
}

/// The EC cannot sustain arbitrary low speeds: explicit off, otherwise >=25%.
pub fn effective_percent(percent: u8) -> u8 {
    if percent == 0 {
        0
    } else {
        percent.clamp(25, 100)
    }
}
#[cfg(test)]
mod off_tests {
    use super::*;
    #[test]
    fn off_is_distinct_from_minimum_running_speed() {
        assert_eq!(effective_percent(0), 0);
        assert_eq!(effective_percent(1), 25);
        assert_eq!(effective_percent(25), 25);
    }
}
