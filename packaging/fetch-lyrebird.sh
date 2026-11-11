#!/bin/sh
# Fetches lyrebird from the Tor expert bundle, checking the Tor Browser
# developers' signature. usage: fetch-lyrebird.sh <platform> (linux-x86_64,
# windows-x86_64, macos-aarch64…); TOR_BUNDLE is the version. Puts lyrebird
# (lyrebird.exe on Windows) in the current folder.
set -eu
f=tor-expert-bundle-$1-$TOR_BUNDLE.tar.gz
u=https://archive.torproject.org/tor-package-archive/torbrowser/$TOR_BUNDLE/$f
curl -fsSLO "$u" -O "$u.asc"
GNUPGHOME=$(mktemp -d); export GNUPGHOME
gpg --auto-key-locate nodefault,wkd --locate-keys torbrowser@torproject.org
gpg --fingerprint EF6E286DDA85EA2A4BA7DE684E2C6E8793298290
gpg --verify "$f.asc" "$f"
mkdir -p tb && tar xzf "$f" -C tb
cp "$(find tb -name 'lyrebird' -type f -o -name 'lyrebird.exe' -type f | head -1)" .
