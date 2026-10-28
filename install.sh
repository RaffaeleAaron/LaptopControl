#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $EUID -ne 0 ]]; then
  echo "Run: sudo ./install.sh <your-login-name>" >&2
  exit 1
fi
login_name=${1:-${SUDO_USER:-}}
if [[ -z "$login_name" || "$login_name" == root ]]; then echo "Specify the desktop user's login name." >&2; exit 1; fi
user_group=$(id -gn -- "$login_name")
if [[ ! "$user_group" =~ ^[a-zA-Z0-9_-]+$ ]]; then echo "Unsupported group name" >&2; exit 1; fi
login_home=$(getent passwd "$login_name" | cut -d: -f6)
if [[ -z "$login_home" || ! -d "$login_home" ]]; then echo "Cannot locate desktop user's home directory" >&2; exit 1; fi
if ps -u "$login_name" -o comm=,stat= | awk '$1 == "laptop-control" && $2 !~ /^Z/ {live=1} END {exit !live}'; then
  echo 'Save your camera edits and close Laptop Control before updating.' >&2; exit 1
fi
for binary in laptop-control raf-fand; do test -x "target/release/$binary"; done
if systemctl is-active --quiet tccd.service || systemctl is-active --quiet fancontrol.service; then
  echo "Stop your existing fan service before installing Laptop Control." >&2; exit 1
fi
for dependency in /usr/bin/python3 /usr/bin/v4l2loopback-ctl /usr/bin/kdialog; do test -x "$dependency" || { echo "Missing $dependency" >&2; exit 1; }; done
/usr/bin/python3 -c 'import gi, cairo; gi.require_version("Gst", "1.0"); gi.require_version("Rsvg", "2.0"); gi.require_version("PangoCairo", "1.0"); from gi.repository import Gst, Rsvg, PangoCairo' || exit 1
/usr/sbin/modinfo v4l2loopback >/dev/null || { echo 'Install the v4l2loopback module for this kernel first.' >&2; exit 1; }
# A GUI/camera-only update need not interrupt a running, identical fan daemon.
if ! cmp -s target/release/raf-fand /usr/local/libexec/raf-fand; then
  if systemctl is-active --quiet raf-fand.service; then systemctl stop raf-fand.service; fi
  install -Dm755 target/release/raf-fand /usr/local/libexec/raf-fand
fi
install -Dm755 target/release/laptop-control /usr/local/bin/laptop-control
# Retire an earlier per-user executable without leaving its launcher on an old build.
if [[ -f "$login_home/.local/bin/laptop-control" || -L "$login_home/.local/bin/laptop-control" ]]; then
  ln -sfn /usr/local/bin/laptop-control "$login_home/.local/bin/laptop-control"
  chown -h "$login_name:$user_group" "$login_home/.local/bin/laptop-control"
fi
ln -sfn /usr/local/bin/laptop-control /usr/local/bin/laptop-fan-control
ln -sfn /usr/local/bin/laptop-control /usr/local/bin/raf-fans
install -Dm644 packaging/laptop-control.desktop /usr/local/share/applications/laptop-control.desktop
install -Dm644 assets/laptop-control.png /usr/local/share/icons/hicolor/256x256/apps/laptop-control.png
install -Dm644 assets/laptop-control.ico /usr/local/share/laptop-control/laptop-control.ico
install -m644 helpers/camera.py helpers/virtual_output.py helpers/control_service.py helpers/tray.py vendor/cameractrls.py vendor/cameractrls-LICENSE vendor/COPYING vendor/COPYING.LESSER LICENSE /usr/local/share/laptop-control/
install -m644 assets/README.md /usr/local/share/laptop-control/ICON-NOTICE.md
install -m644 vendor/README.md /usr/local/share/laptop-control/THIRD-PARTY.md
install -Dm755 packaging/laptop-control-camera-device /usr/local/libexec/laptop-control-camera-device
install -Dm644 packaging/laptop-control-camera.service /etc/systemd/system/laptop-control-camera.service
printf 'SUBSYSTEM=="video4linux", ATTR{name}=="Laptop Control Camera", GROUP="%s", MODE="0660", TAG+="uaccess"\n' "$user_group" > /etc/udev/rules.d/70-laptop-control-camera.rules
udevadm control --reload-rules
rm -f /usr/local/share/applications/raf-fans.desktop /usr/local/share/applications/laptop-fan-control.desktop
if [[ ! -d "$login_home/.config/autostart" ]]; then
  install -d -m755 -o "$login_name" -g "$user_group" "$login_home/.config/autostart"
fi
rm -f "$login_home/.config/autostart/laptop-fan-control.desktop"
install -m644 -o "$login_name" -g "$user_group" packaging/laptop-control-autostart.desktop "$login_home/.config/autostart/laptop-control.desktop"
install -d -m755 -o "$login_name" -g "$user_group" "$login_home/.config/systemd" "$login_home/.config/systemd/user" "$login_home/.config/systemd/user/default.target.wants"
sed -e 's|@HELPER@|/usr/local/share/laptop-control/camera.py|g' -e 's|@GUI@|/usr/local/bin/laptop-control|g' packaging/laptop-control.service > "$login_home/.config/systemd/user/laptop-control.service"
chmod 644 "$login_home/.config/systemd/user/laptop-control.service"
chown "$login_name:$user_group" "$login_home/.config/systemd/user/laptop-control.service"
ln -sfn ../laptop-control.service "$login_home/.config/systemd/user/default.target.wants/laptop-control.service"
chown -h "$login_name:$user_group" "$login_home/.config/systemd/user/default.target.wants/laptop-control.service"
sed "s/@GROUP@/$user_group/" packaging/raf-fand.service > /etc/systemd/system/raf-fand.service
chmod 644 /etc/systemd/system/raf-fand.service
install -Dm755 packaging/raf-fans-sleep /usr/lib/systemd/system-sleep/raf-fans
systemctl daemon-reload
systemctl enable --now raf-fand.service laptop-control-camera.service
udevadm trigger --subsystem-match=video4linux
udevadm settle
login_uid=$(id -u -- "$login_name")
if [[ -S "/run/user/$login_uid/bus" ]]; then
  runuser -u "$login_name" -- env XDG_RUNTIME_DIR="/run/user/$login_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$login_uid/bus" systemctl --user daemon-reload
  runuser -u "$login_name" -- env XDG_RUNTIME_DIR="/run/user/$login_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$login_uid/bus" systemctl --user restart laptop-control.service
fi
if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -f -t /usr/local/share/icons/hicolor >/dev/null 2>&1 || true; fi
if command -v update-desktop-database >/dev/null; then update-desktop-database /usr/local/share/applications; fi
echo "Installed Laptop Control. Saved profiles restore automatically. Camera capture starts only when requested in the app."
