#!/bin/bash
set -euo pipefail
[ "$(pwd -P)" = /work ] && [ -f /opt/kindle-toolchain/env.sh ]
umask 022
apt_root=/work/apt-arm
for tree in "$apt_root" /work/sdk /work/runtime; do [ ! -e "$tree" ] && [ ! -L "$tree" ]; done
mkdir -p "$apt_root/lists/partial" "$apt_root/cache/archives/partial" "$apt_root/etc" /work/sdk /work/runtime
touch "$apt_root/status"
for suite in noble noble-updates noble-security; do
    printf 'deb [arch=armhf signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://ports.ubuntu.com/ubuntu-ports %s main universe\n' "$suite"
done >"$apt_root/etc/sources.list"
options=(-o APT::Sandbox::User=root -o APT::Architecture=armhf -o APT::Architectures::=armhf
    -o Dir::Etc::sourcelist="$apt_root/etc/sources.list" -o Dir::Etc::sourceparts=-
    -o Dir::State::lists="$apt_root/lists" -o Dir::State::status="$apt_root/status"
    -o Dir::Cache="$apt_root/cache" -o Acquire::Languages=none -o Acquire::Retries=2)
apt-get "${options[@]}" update
apt-get "${options[@]}" --no-install-recommends --download-only -y install \
    libtesseract5:armhf libx11-6:armhf libxtst6:armhf tesseract-ocr-eng tesseract-ocr-chi-sim
cd /work/sdk
apt-get "${options[@]}" download libx11-dev:armhf libxtst-dev:armhf x11proto-dev libtesseract-dev:armhf
for package in ./*.deb; do dpkg-deb -x "$package" .; done
for package in "$apt_root"/cache/archives/*.deb; do
    dpkg-deb -x "$package" /work/runtime
    dpkg-deb -W --showformat='${Package}\t${Architecture}\t${Version}\n' "$package"
done | sort >"$apt_root/selected-runtime.tsv"
