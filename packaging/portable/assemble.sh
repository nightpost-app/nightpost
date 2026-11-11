#!/bin/sh
# The folder to copy onto a key formatted exFAT (readable by Linux, macOS
# and Windows), from a release's archives and the Mac app built by hand:
#   packaging/portable/assemble.sh <out> <nightpost-linux-x86_64.tar.gz> <nightpost-windows-x86_64.zip> [Nightpost.app]
set -eu
out=$1 linux=$2 windows=$3 mac=${4:-}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out/linux" "$out/windows" "$out/data"
tar xzf "$linux" -C "$out/linux" --strip-components=1
unzip -qo "$windows" -d "$out/windows"
if [ -n "$mac" ]; then
    mkdir -p "$out/mac"
    cp -R "$mac" "$out/mac/Nightpost.app"
fi
cp "$here/Nightpost Linux.sh" "$here/Nightpost Mac.command" "$here/Nightpost Windows.bat" "$here/LISEZ-MOI.txt" "$here/README.txt" "$out/"
chmod +x "$out/Nightpost Linux.sh" "$out/Nightpost Mac.command"
echo "Done: copy what is in $out to the root of the key."
