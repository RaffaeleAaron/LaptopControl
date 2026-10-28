#!/usr/bin/env bash
set -euo pipefail
if [[ $EUID -ne 0 ]]; then echo 'Run: sudo ./uninstall.sh' >&2; exit 1; fi
login_name=${1:-${SUDO_USER:-}}
if [[ -n "$login_name" && "$login_name" != root ]]; then
  login_home=$(getent passwd "$login_name" | cut -d: -f6)
  if [[ -n "$login_home" ]]; then
    login_uid=$(id -u -- "$login_name")
    if [[ -S "/run/user/$login_uid/bus" ]]; then
      runuser -u "$login_name" -- env XDG_RUNTIME_DIR="/run/user/$login_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$login_uid/bus" systemctl --user disable --now laptop-control.service || true
    fi
    rm -f "$login_home/.config/systemd/user/laptop-control.service" "$login_home/.config/systemd/user/default.target.wants/laptop-control.service"
    rm -f "$login_home/.config/autostart/laptop-fan-control.desktop" "$login_home/.config/autostart/laptop-control.desktop"
    rm -f "$login_home/.local/bin/laptop-control" "$login_home/.local/share/applications/laptop-control.desktop" "$login_home/.local/share/icons/hicolor/256x256/apps/laptop-control.png"
    rm -f "$login_home/.local/share/laptop-control/"{camera.py,virtual_output.py,control_service.py,tray.py,cameractrls.py,cameractrls-LICENSE,COPYING,COPYING.LESSER,LICENSE,ICON-NOTICE.md,THIRD-PARTY.md}
    if [[ -S "/run/user/$login_uid/bus" ]]; then
      runuser -u "$login_name" -- env XDG_RUNTIME_DIR="/run/user/$login_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$login_uid/bus" systemctl --user daemon-reload
    fi
  fi
fi
systemctl disable --now raf-fand.service laptop-control-camera.service
for name in /sys/class/video4linux/video*/name; do
  [[ -f "$name" ]] || continue
  if [[ $(<"$name") == "Laptop Control Camera" ]]; then v4l2loopback-ctl delete "/dev/$(basename "$(dirname "$name")")" || true; fi
done
rm -f /etc/systemd/system/laptop-control-camera.service /usr/local/libexec/laptop-control-camera-device /etc/udev/rules.d/70-laptop-control-camera.rules /usr/local/bin/laptop-control /usr/local/share/applications/laptop-control.desktop /usr/local/share/icons/hicolor/256x256/apps/laptop-control.png
rm -f /usr/local/share/laptop-control/{camera.py,virtual_output.py,control_service.py,tray.py,cameractrls.py,cameractrls-LICENSE,COPYING,COPYING.LESSER,LICENSE,ICON-NOTICE.md,THIRD-PARTY.md,laptop-control.ico}
rm -f /etc/systemd/system/raf-fand.service /usr/lib/systemd/system-sleep/raf-fans /usr/local/libexec/raf-fand /usr/local/bin/raf-fans /usr/local/bin/laptop-fan-control /usr/local/share/applications/raf-fans.desktop /usr/local/share/applications/laptop-fan-control.desktop /run/raf-fans-resume
systemctl daemon-reload
echo 'Removed Laptop Control. Saved curve settings remain in /var/lib/raf-fans.'
