# LaptopControl

A native Rust/egui application for Linux laptop fan control, webcam settings,
live camera scenes, and system monitoring. Made by **R. A. Giampaolo**.

## Why I built it

I built Laptop Control because I wasn't happy with TUXEDO's controller for camera,
power, and fan management. I wanted one lightweight application with independent
fan curves, camera settings that I could save for different lighting conditions,
and a virtual webcam with my own scenes and logos.

I used **OpenAI Codex** to help develop, refine, test, and clean up the software.
The design and feature choices came from using it on my own laptop.

Power features currently show battery flow, platform mode and processor usage;
the app does not change CPU power limits or performance policies.

## Hardware and desktop support

The fan backend is specifically implemented for the **TUXEDO InfinityBook Pro
Gen7 MK2**, board `PH6AG01_PH6AQ71_PH6AQI1`, on Linux x86_64. It uses
`/dev/tuxedo_io` from tuxedo-drivers. It is not a generic laptop EC driver. See [SECURITY.md](SECURITY.md) for the trust boundaries and security limitations.
Development and hardware checks used Fedora KDE and an Intel/NVIDIA hybrid laptop.
Camera composition currently requires an Intel render device. KDE supplies the
palette, native blur, file picker, and system tray integration.

## Features

- Independent CPU/dGPU curves, manual fan outputs, and Firmware/Office/Gaming profiles.
- Draggable curve points, per-fan temperature sources, **0–60 second temperature
  averaging**, and a separate configurable stepped speed ramp.
- Explicit opt-in NVIDIA telemetry: no NVIDIA commands during GPU sleep.
- Live webcam controls and named camera presets such as Backlight, Dark, Office
  or Home, with editable names and saving during capture.
- Unlimited named scenes with PNG/SVG logos and text layers, composed on the iGPU.
- Independent virtual-camera output that remains available with the window closed.
- Tray menus for fan profiles, saved scenes, capture and virtual output.
- Collapsible navigation, KDE/dark/light themes, custom accent and opacity,
  and a dedicated system-monitor page.

## Build and install

Use Rust **1.92 or later**, Cargo, and the committed lockfile:

```sh
cargo build --release --locked
```

Runtime dependencies include tuxedo-drivers, systemd, Python 3/PyGObject, Bubblewrap,
Cairo/Pango, librsvg, GStreamer with OpenGL/VA/video4linux2 plugins,
`v4l2loopback` and its tools, `v4l2-ctl`, and KDE `kdialog`. Install these using
packages appropriate to your distribution and running kernel before installation.

For first installation, or an update that changes the fan daemon:

```sh
sudo ./install.sh "$USER"
```

The fan backend enforces its hardware allowlist. The installer rejects competing
fan services, installs the daemon,
launcher and tray service, and creates a dedicated **Laptop Control Camera**.
It preserves the existing OBS loopback device. The GUI and camera pipeline run
as the desktop user; only the fan service and loopback-device setup need root.
Saved fan profiles are retained when updating.
The full installer also redirects an existing per-user executable to the updated
system binary, so its launcher cannot remain on an older GUI/helper combination.

For later **GUI/camera-only** updates on an already configured system:

```sh
./install-user.sh
```

This installs under `~/.local`, overrides the application launcher for your user,
and restarts the camera/tray service without restarting the fan daemon. Close
and save your work first; a camera-service restart stops active capture. Use the
full installer for version 0.4.1's fan-service authorization and installer hardening. A GUI-only update
cannot add those protections to an older system daemon.

Open **Laptop Control** from the KDE launcher or tray. The tray starts at login;
the physical camera never starts automatically. The legacy `raf-fans` and
`laptop-fan-control` commands remain compatibility aliases.

## Fan profiles and averaging

Firmware delegates to the embedded controller. Office and Gaming each hold two
independent fan configurations. Selecting a profile applies its saved settings;
editing a curve is a draft until **Save profile** is pressed.

Each curve can use CPU temperature, GPU hwmon temperature, or explicit
`nvidia-smi` telemetry. Either fan may follow any source. If the selected source
is asleep or unreadable, that fan uses its configured fallback output, including
0%. Missing CPU telemetry releases both fans to firmware because this hardware
has a shared firmware-restoration command.

**Temperature averaging**, below each temperature graph, sets a rolling window
from **0 to 60 seconds**. Zero uses the current reading. Try **5 seconds** to
reduce the response to a brief temperature spike. Existing configurations migrate
with averaging off, preserving their previous behavior. The daemon averages only
available readings in that window, warms up from the first fresh sample, and
clears history when the source is missing, changed, or a profile is reapplied.
It reuses the existing one-second sampling cadence and a small running sum;
there is no extra sensor polling or background averaging thread.

**Fan transition** separately controls the sound of speed changes: the default
is four steps over two seconds. Set duration to zero for immediate output changes.
Manual settings and first startup/recovery use the current target immediately.
Power-source changes reassert the current output without cycling through firmware.

A **raw** physical or selected curve temperature of **90°C** forces the relevant
fan to 100% immediately, bypassing both averaging and the speed ramp. A readable
hot physical component also overrides manual output. 0% requests fan-off; this
EC has a 25% minimum while running, so positive requests below 25% become 25%.
Displayed outputs are requested percentages, not measured RPM.

NVIDIA sampling requires PCI **D0** and runtime **active**, rechecked before an
external query. D3hot, D3cold, transitional and unknown states skip queries.
`nvidia-settings` is never used. Choose the dGPU polling interval (1–300 seconds)
in Settings → Telemetry. SMI is requested only by an active SMI curve or the
explicitly enabled Gaming utilization monitor; queries are shared and timed out.

## Camera controls, presets and saving

The native controls use the bundled [cameractrls backend](https://github.com/soyersoyer/cameractrls).
Supported hardware controls are exposed according to driver metadata. Sliders
apply on release; inactive controls stay disabled while their automatic mode is on.
Controls requiring the camera to reopen are disabled during capture.

Under Capture → **Camera presets**:

1. Choose **New preset**, then edit its name, for example Backlight or Office.
2. Adjust exposure, white balance, brightness and other supported controls live.
3. Press **Save preset** or the top **Save changes** button.
4. Select a saved preset and press **Load preset** to apply it without restarting
   capture. Presets belong to the physical camera they were saved for.

Saving reads fresh hardware values after pending control operations, updates the
selected preset, and atomically saves the camera configuration. Automatic modes
restore before dependent manual controls. Inactive, read-only, unavailable,
unsafe and reopen-only controls are excluded; optional restoration failures are
reported without aborting the stream. Preset names can be edited at any time.

Closing a window with camera changes offers **Save and close**, **Close without
saving**, or **Cancel**. Saving must succeed before the window closes. Tray Quit
uses the same decision before it stops the background service. Forced service
termination or system shutdown cannot offer an interactive save dialog.

## Scenes and virtual output

The preview sits above Capture and Scenes & Logos. Below the latter, Scenes are
stacked above Layers on the left, with the inspector on the right. Add, duplicate,
rename and delete scenes; add image/text layers, change their order or visibility,
and drag/resize them in the preview. SVG and text rasterization is cached in a resource-limited, filesystem/network-isolated worker; normal
composition uses Intel GPU textures. Save changes persists all scenes.

**Start camera** and **Virtual camera output** are independent:

- Start camera opens the physical camera and runs the compositor.
- Enabled virtual output advertises **Laptop Control Camera** to Teams and
  browsers. When capture is off it supplies black standby frames.
- Enable output, start capture, then choose Laptop Control Camera in the meeting
  application. Refresh its camera list if needed.
- Stop capture and disable output before changing output dimensions/frame rate.

Virtual output starts enabled on first installation and remembers explicit on/off
choices. Closing the window releases the interface; enabled output and active
capture continue through the tray service. Reopening during capture recovers the
active scene book from memory. Save to retain changes across service restarts.
Tray **Quit Laptop Control** stops camera processing/output; the fan daemon
continues independently.

Preview follows the chosen capture frame rate and pauses when the Webcam page or
window is closed. Frames use one transient RAM-backed buffer, with no recording
history. Virtual standby reuses one cached black frame once per second and opens
neither the physical camera nor a GPU device.

## Appearance and monitoring

Settings contains KDE/Black/White themes, accent color and opacity controls,
dGPU polling, and optional CPU/power monitoring. The default is 85% background
opacity with native KWin blur when supported. Settings shows the credit
**Made by R. A. Giampaolo** at the bottom right.

Monitors shows system identity, battery charge/discharge watts, platform mode,
and CPU utilization. Battery flow is not total wall-power consumption. Sampling
pauses outside that page. Gaming GPU usage is explicitly opt-in and shares the
D0-gated telemetry worker.

## Configuration and services

| Component | Location |
| --- | --- |
| Fan profiles and averaging | `/var/lib/raf-fans/profiles.json` |
| Camera controls, presets and scenes | `~/.config/laptop-control/camera.json` |
| Appearance and monitoring | `~/.config/laptop-control/settings.json` |
| Virtual-output preference | `~/.config/laptop-control/virtual-output.json` |
| Local camera IPC and preview | `$XDG_RUNTIME_DIR/laptop-control/` |

User configuration respects `XDG_CONFIG_HOME`. Settings directories must be owned by the desktop user, private, and not symlinks; settings files are written as 0600. Legacy settings and single-scene
camera files migrate automatically. The camera socket is user-only (0600), inside
a 0700 runtime directory. The fan socket is restricted to the configured group and peer UID (root or the configured desktop user).

```sh
systemctl status raf-fand.service
journalctl -u raf-fand.service -b
systemctl --user status laptop-control.service
journalctl --user -u laptop-control.service -b
```

`laptop-control-camera.service` is the separate root oneshot that creates only the
loopback device. Stopping `raf-fand.service` requests firmware control. The fan
worker has a watchdog and shutdown restoration; these userspace measures cannot
guarantee recovery from kernel, driver or firmware failure.

## Development and tests

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
XDG_RUNTIME_DIR=/tmp /usr/bin/python3 -m unittest discover -s tests -p 'test_*.py'
./target/release/laptop-control --demo
```

Hardware-free tests cover legacy migrations, camera preset persistence, close
choices, averaging and spike expiry, thermal bypass, profile persistence, source
selection, GPU sleep gating, smoothing and IPC responsiveness. Unix-socket tests
need permission to bind local sockets.

Host camera checks must run serially and require the installed dependencies.
Stop capture and disable **Virtual camera output** in the app before running them:

```sh
python3 tests/camera_smoke.py
python3 tests/camera_smoke.py --real
python3 tests/service_smoke.py
python3 tests/render_ui.py
python3 scripts/check-advisories.py
```

The default camera test uses synthetic video. `--real` briefly opens the physical
webcam and discards frames. Rendering uses an isolated virtual KWin display,
without unlocking or interacting with the desktop. These checks verify the
pipeline, Intel rendering, overlays and V4L2 output; they do not join a real
Teams/browser meeting. See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution scope.

## License and attribution

Project-authored source is [MIT licensed](LICENSE). The unmodified cameractrls
backend is **LGPL-3.0-or-later**; its source, attribution and full license texts
are in [vendor/](vendor/README.md). Supplied processor artwork has separate,
unestablished redistribution terms; see [assets/README.md](assets/README.md).
Generated previews, caches, local environment files and build outputs are ignored.

Overlay input limits are 8 MiB per image, 4096 pixels per input dimension,
1280-pixel cached raster output, and 8192 text characters. SVG linked files,
external stylesheets, DTDs and entities are unsupported; embed images/fonts in
the document. Invalid or oversized layers report a warning instead of opening
resources outside the sandbox. Bubblewrap/user namespaces are required; there is
no unsandboxed fallback. Pillow is not used by the application.

## Uninstall

```sh
sudo ./uninstall.sh "$USER"
```

This removes the installed application and services and requests firmware fan
control. It removes only this application's unused virtual device, preserving
OBS. Saved user settings and fan profiles remain.
