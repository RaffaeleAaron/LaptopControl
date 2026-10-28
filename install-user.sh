#!/usr/bin/env bash
# GUI/camera-only update for an already configured Laptop Control system.
# No root access, fan-service restart, module reload, or desktop unlock needed.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $EUID -eq 0 ]]; then echo 'Run this GUI update as your desktop user.' >&2; exit 1; fi
if ps -u "$UID" -o comm=,stat= | awk '$1 == "laptop-control" && $2 !~ /^Z/ {live=1} END {exit !live}'; then
  echo 'Save your camera edits and close Laptop Control before updating.' >&2; exit 1
fi
if ! systemctl is-active --quiet raf-fand.service; then
  echo 'Set up the system components first: sudo ./install.sh <login-name>' >&2; exit 1
fi
test -x target/release/laptop-control
/usr/bin/python3 -c 'import gi, cairo; gi.require_version("Gst", "1.0"); gi.require_version("Rsvg", "2.0"); gi.require_version("PangoCairo", "1.0"); from gi.repository import Gst, Rsvg, PangoCairo'
user_data_root=${XDG_DATA_HOME:-"$HOME/.local/share"}
user_config_root=${XDG_CONFIG_HOME:-"$HOME/.config"}
user_bin_root="$HOME/.local/bin"
install -Dm755 target/release/laptop-control "$user_bin_root/laptop-control"
install -Dm644 helpers/camera.py "$user_data_root/laptop-control/camera.py"
install -m644 helpers/virtual_output.py helpers/control_service.py helpers/tray.py vendor/cameractrls.py vendor/cameractrls-LICENSE vendor/COPYING vendor/COPYING.LESSER LICENSE "$user_data_root/laptop-control/"
install -m644 assets/README.md "$user_data_root/laptop-control/ICON-NOTICE.md"
install -m644 vendor/README.md "$user_data_root/laptop-control/THIRD-PARTY.md"
install -Dm644 assets/laptop-control.png "$user_data_root/icons/hicolor/256x256/apps/laptop-control.png"
install -d "$user_data_root/applications" "$user_config_root/autostart" "$user_config_root/systemd/user"
sed -e "s|@HELPER@|$user_data_root/laptop-control/camera.py|g" -e "s|@GUI@|$user_bin_root/laptop-control|g" packaging/laptop-control.service > "$user_config_root/systemd/user/laptop-control.service"
# A same-name per-user desktop file overrides the system launcher cleanly.
sed "s|Exec=/usr/local/bin/laptop-control|Exec=\"$user_bin_root/laptop-control\"|" packaging/laptop-control.desktop > "$user_data_root/applications/laptop-control.desktop"
install -m644 packaging/laptop-control-autostart.desktop "$user_config_root/autostart/laptop-control.desktop"
systemctl --user daemon-reload
systemctl --user enable laptop-control.service
systemctl --user restart laptop-control.service
if command -v update-desktop-database >/dev/null; then update-desktop-database "$user_data_root/applications"; fi
echo 'Updated Laptop Control for your user. Reopen it from the application launcher. The tray starts at login; the fan service was not interrupted.'
