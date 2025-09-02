#!/usr/bin/env bash
set -euo pipefail

# Extract all kVK_* keycode constants from the macOS SDK Events.h header
# and write them as "NAME VALUE" to ./data/keycodes.txt.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_FILE="$SCRIPT_DIR/data/keycodes.txt"

HEADER_PATH="/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/HIToolbox.framework/Versions/A/Headers/Events.h"

if [[ ! -f "$HEADER_PATH" ]]; then
  echo "Events.h not found at: $HEADER_PATH" >&2
  exit 1
fi

mkdir -p "$SCRIPT_DIR/data"

# We intentionally keep the literal value found in the header (e.g., 0x00),
# without converting formats.

perl -ne '
  if (/\b(kVK_[A-Za-z0-9_]+)\s*=\s*([^,]+)/) {
    my ($name, $val) = ($1, $2);
    $val =~ s/^\s+//; $val =~ s/\s+$//;
    print "$name $val\n";
  }
' "$HEADER_PATH" > "$OUT_FILE"

echo "Wrote $(wc -l < "$OUT_FILE") keycodes to $OUT_FILE"
