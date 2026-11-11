#!/bin/sh
# Starts Nightpost from this key: its data stay in data/, next to this file.
here=$(cd "$(dirname "$0")" && pwd)
NIGHTPOST_PORTABLE=1 NIGHTPOST_DIR="$here/data" exec "$here/linux/nightpost-gui" "$@"
