#!/usr/bin/env bash
# Package only source and distribution notices, never local state or previews.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
release_version=$(sed -nE 's/^version = "([^"]+)"/\1/p' Cargo.toml)
archive_name="laptop-control-$release_version"
mkdir -p dist
sources=(.gitattributes .gitignore Cargo.toml Cargo.lock README.md LICENSE
  CONTRIBUTING.md SECURITY.md install.sh install-user.sh uninstall.sh
  src helpers packaging scripts tests vendor assets)
if [[ -d .github ]]; then sources+=(.github); fi
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='@0' \
  --exclude='__pycache__' --exclude='*.pyc' --exclude='*.pyo' \
  --transform="s|^|$archive_name/|" \
  -czf "dist/$archive_name-source.tar.gz" \
  "${sources[@]}"
echo "dist/$archive_name-source.tar.gz"
