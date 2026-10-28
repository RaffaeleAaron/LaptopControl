# Security scope

LaptopControl is a local desktop application, not an Internet service. A root
fan daemon accesses the supported EC driver; the GUI, tray, camera and overlay
workers run as the desktop user. Anyone who can run arbitrary code as that same
user already has access to that user's session and camera: the desktop account
is the principal trust boundary.

## Hardening in 0.4.1

- Root installation/removal never writes into the desktop user's home as root.
  Per-user setup runs after dropping to that user's UID. This prevents malicious
  home-directory symlinks from redirecting privileged writes.
- The fan daemon checks Linux peer credentials: only root and the explicitly
  configured desktop UID may connect, even if its primary group is shared.
  Requests are at most 8 KiB and have an absolute 100 ms read deadline. Lock files
  are private and cannot be symlinks.
- Camera IPC checks peer UID, uses a private runtime directory/socket, permits at
  most eight connections, caps each command at 4 MiB, and bounds partial-request
  time and pending work per client. Capture dimensions/rates are validated before
  opening devices or allocating frames.
- Overlay rendering runs only on cache misses, in a Bubblewrap user/filesystem/
  network/PID namespace. It sees the chosen image, system libraries/fonts, optional
  user font directories, and one output file. Home files, other image-directory
  files, the camera and GPU devices are not exposed to the renderer.
- Overlay subprocesses have address-space, CPU-time, wall-time, output-size and
  descriptor limits. SVG external files/URLs, DTDs, entities, includes and external
  stylesheets are rejected. SVGs must embed resources. Large canvases and long
  text are rejected with a visible layer warning.
- Every generated overlay is normalized through the bounded Rust PNG/JPEG decoder
  before it enters the host media pipeline; native SVG/text output is not trusted.
  Tray icons use the same decoder. Pillow is not an
  application runtime dependency. Only PNG/JPEG decoding is exposed.
- User settings use private directories/files and exclusive temporary files,
  followed by atomic replacement. The user service also has memory/task limits
  and cannot create Internet sockets or gain privileges.

The configured fan UID is stored in the root-owned system unit as
`LAPTOP_CONTROL_UID`. The full installer updates this; copying only the daemon
binary into an older installation is unsupported. Firmware restore/probe commands
remain available for root without a GUI.

## Dependency checks

Run `python3 scripts/check-advisories.py` to query OSV with package names/versions
from Cargo.lock. It sends no source, settings, credentials or device information.
A successful scan means no matching *published* advisory was returned at that
moment; it does not establish that the dependencies have no vulnerabilities.

Keep the distribution's kernel, tuxedo driver, Bubblewrap, GStreamer, librsvg,
Cairo, GLib and graphics drivers security-updated. Distribution backports may not
change an upstream version string. The source manifest cannot audit those system
packages. Old/unsupported native parsers should not be used.

## Limits of this review

A passing test suite or advisory scan is not a security certification. Kernel,
GPU/media drivers, compositor/session security, hardware/firmware, malicious
same-UID processes and future native-parser bugs remain outside a zero-risk
claim. The overlay sandbox reduces parser impact; it is not a proof against
kernel/sandbox escapes. Large collections of legitimate layers are still subject
to finite CPU/GPU/memory budgets.

## Reporting

Use a private GitHub vulnerability report if the repository offers that option.
Otherwise contact the maintainer privately before publishing an exploit. Provide
synthetic reproduction steps, not private camera frames, secrets or personal
configuration. This document does not promise a guaranteed response time.
