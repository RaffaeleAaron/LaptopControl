# Contributing

Build with Rust 1.92 or later. Keep hardware support explicit: the current fan
backend supports one TUXEDO board family, not arbitrary EC registers or laptops.
Preserve the raw-temperature emergency override and firmware fallback.

Before submitting changes, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
XDG_RUNTIME_DIR=/tmp /usr/bin/python3 -m unittest discover -s tests -p 'test_*.py'
bash -n install.sh install-user.sh uninstall.sh packaging/laptop-control-user-setup
python3 scripts/check-advisories.py
```

Unix-socket tests need a normal Linux user session. Camera and rendering smoke
tests need the host dependencies and Intel graphics described in the README;
they do not run in CI. Run camera tests serially: they share the dedicated virtual
camera. `--real` briefly opens the physical camera and discards frames.

Include migration coverage for settings changes and regression coverage for fan
protection or camera lifecycle changes. Keep hardware operations out of UI drawing
code. Do not include local settings, recordings, private logo paths, build outputs,
or personal screenshots in contributions. Keep the vendored backend unmodified
and retain its source attribution and licenses.

Security tests require Bubblewrap and a built Rust application binary. If using
an external Cargo target directory, set `LAPTOP_CONTROL_RASTER_BINARY` to that
`laptop-control` binary. `tests/privileged_install_smoke.py` is a root-only,
temporary-fixture test for user/home symlink privilege boundaries. It never
installs services or modifies real user settings.
