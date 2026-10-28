#!/usr/bin/env bash
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
if [[ $EUID -ne 0 ]]; then echo 'Run: sudo ./uninstall.sh' >&2; exit 1; fi
script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
login_name=${1:-${SUDO_USER:-}}
if [[ -n "$login_name" && "$login_name" != root ]]; then
  login_home=$(getent passwd "$login_name" | cut -d: -f6)
  if [[ -n "$login_home" ]]; then
    login_uid=$(id -u -- "$login_name")
    if [[ $login_uid -eq 0 ]]; then echo 'Choose a non-root desktop user.' >&2; exit 1; fi
    runuser -u "$login_name" -- env HOME="$login_home" XDG_CONFIG_HOME="$login_home/.config" XDG_DATA_HOME="$login_home/.local/share" XDG_RUNTIME_DIR="/run/user/$login_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$login_uid/bus" /usr/bin/bash -s -- --remove < "$script_root/packaging/laptop-control-user-setup" || echo "User cleanup was incomplete; continuing system removal." >&2

  fi
fi
systemctl disable --now raf-fand.service laptop-control-camera.service
for name in /sys/class/video4linux/video*/name; do
  [[ -f "$name" ]] || continue
  if [[ $(<"$name") == "Laptop Control Camera" ]]; then v4l2loopback-ctl delete "/dev/$(basename "$(dirname "$name")")" || true; fi
done
rm -f /etc/systemd/system/laptop-control-camera.service /usr/local/libexec/laptop-control-camera-device /etc/udev/rules.d/70-laptop-control-camera.rules /usr/local/bin/laptop-control /usr/local/share/applications/laptop-control.desktop /usr/local/share/icons/hicolor/256x256/apps/laptop-control.png
rm -f /usr/local/share/laptop-control/{camera.py,virtual_output.py,control_service.py,tray.py,security.py,render_asset.py,laptop-control-autostart.desktop,laptop-control.service,cameractrls.py,cameractrls-LICENSE,COPYING,COPYING.LESSER,LICENSE,ICON-NOTICE.md,THIRD-PARTY.md,laptop-control.ico}
rm -f /etc/systemd/system/raf-fand.service /usr/lib/systemd/system-sleep/raf-fans /usr/local/libexec/raf-fand /usr/local/bin/raf-fans /usr/local/bin/laptop-fan-control /usr/local/share/applications/raf-fans.desktop /usr/local/share/applications/laptop-fan-control.desktop /run/raf-fans-resume
rm -f /usr/local/libexec/laptop-control-user-setup
systemctl daemon-reload
echo 'Removed Laptop Control. Saved curve settings remain in /var/lib/raf-fans.'
