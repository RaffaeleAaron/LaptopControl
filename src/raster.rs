//! Bounded, headless Rust image decoding. Never starts the GUI or opens a GPU.
use image::{GenericImageView, ImageFormat, ImageReader, Limits};
use std::{
    fs::File,
    io::{self, BufReader, Write},
    path::Path,
};

fn decode(path: &Path) -> Result<image::DynamicImage, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 8 * 1024 * 1024 {
        return Err("Use a regular image file no larger than 8 MiB".into());
    }
    let format = match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => ImageFormat::Png,
        Some("jpg" | "jpeg") => ImageFormat::Jpeg,
        _ => return Err("Only PNG and JPEG raster images are accepted".into()),
    };
    let mut reader = ImageReader::with_format(BufReader::new(file), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}

fn isolate_limits() -> io::Result<()> {
    // Only called in the headless child, before threads or parsers are started.
    for (resource, soft, hard) in [
        (libc::RLIMIT_AS, 768 * 1024 * 1024, 768 * 1024 * 1024),
        (libc::RLIMIT_CPU, 3, 4),
        (libc::RLIMIT_FSIZE, 16 * 1024 * 1024, 16 * 1024 * 1024),
        (libc::RLIMIT_NOFILE, 64, 64),
    ] {
        let limit = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // SAFETY: valid resource identifier and pointer to an initialized rlimit.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: PR_SET_NO_NEW_PRIVS takes scalar arguments, no pointers.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    isolate_limits()?;
    if args.first().map(String::as_str) == Some("--tray-icon") && args.len() == 2 {
        let image = decode(Path::new(&args[1]))?
            .thumbnail_exact(32, 32)
            .to_rgba8();
        let mut bytes = Vec::with_capacity(4096);
        for pixel in image.pixels() {
            bytes.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        io::stdout().write_all(&bytes)?;
    } else if args.len() == 3 && args[0] == "--render-raster" {
        let image = decode(Path::new(&args[1]))?;
        let image = if image.width() > 1280 || image.height() > 1280 {
            image.thumbnail(1280, 1280)
        } else {
            image
        };
        image.save_with_format(&args[2], ImageFormat::Png)?;
        let (width, height) = image.dimensions();
        println!("{}", serde_json::json!({"width":width,"height":height}));
    } else {
        return Err("Invalid image-rendering arguments".into());
    }
    Ok(())
}
