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
if [ -n "$lyrebird" ]; then
  cp "$lyrebird" "$app/Contents/MacOS/lyrebird"
  cp ../../docs/lyrebird.txt "$app/Contents/Resources/LICENSE-lyrebird.txt"
fi
cp ../../LICENSE "$app/Contents/Resources/LICENSE.txt"
# The words, in the same files as the other interfaces (Words.swift).
cp ../../crates/core/locales/*.ftl "$app/Contents/Resources/"
cp ../../crates/gui/fonts/SpaceGrotesk.ttf ../../crates/gui/fonts/OFL-SpaceGrotesk.txt "$app/Contents/Resources/"
# The icon, drawn by icon.swift at every size macOS asks for.
icons=build/Nightpost.iconset
rm -rf "$icons" && mkdir -p "$icons"
swift icon.swift build/icon.png
for n in 16 32 128 256 512; do
  sips -z $n $n build/icon.png --out "$icons/icon_${n}x${n}.png" >/dev/null
  sips -z $((n * 2)) $((n * 2)) build/icon.png --out "$icons/icon_${n}x${n}@2x.png" >/dev/null
done
iconutil -c icns "$icons" -o "$app/Contents/Resources/Nightpost.icns"
# Ad hoc: enough to run here; a release needs a Developer ID and notarizing.
# lyrebird keeps the Tor Project's own signature.
codesign --force --sign - "$app/Contents/MacOS/nightpost"
codesign --force --sign - "$app"
# To hand over: unzipped, it opens with a right click, Open (ad hoc signed).
ditto -c -k --keepParent "$app" build/Nightpost-macos.zip
echo "$app"
