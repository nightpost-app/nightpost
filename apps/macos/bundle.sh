#!/bin/bash
# Builds build/Nightpost.app: this interface, with the daemon and lyrebird
# (bridges) beside it, where the daemon looks for lyrebird.
# usage: bundle.sh <nightpost> [lyrebird]
set -euo pipefail
nightpost=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
lyrebird=${2:+$(cd "$(dirname "$2")" && pwd)/$(basename "$2")}
cd "$(dirname "$0")"
swift build -c release
app=build/Nightpost.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp Info.plist "$app/Contents/"
cp .build/release/NightpostMac "$app/Contents/MacOS/"
cp "$nightpost" "$app/Contents/MacOS/nightpost"
if [ -n "$lyrebird" ]; then cp "$lyrebird" "$app/Contents/MacOS/lyrebird"; fi
# Ad hoc: enough to run here; a release needs a Developer ID and notarizing.
# lyrebird keeps the Tor Project's own signature.
codesign --force --sign - "$app/Contents/MacOS/nightpost"
codesign --force --sign - "$app"
echo "$app"
