#!/bin/sh
# Starts Nightpost from this key: its data stay in data/, next to this file.
here=$(cd "$(dirname "$0")" && pwd)
open -n --env NIGHTPOST_PORTABLE=1 --env NIGHTPOST_DIR="$here/data" "$here/mac/Nightpost.app"
