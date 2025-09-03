#!/usr/bin/env bash

# Build a macOS .app bundle for hotki in one go.
# - Builds release binary
# - Generates .icns from the logo in crates/hotki/assets/logo.png
# - Writes Info.plist
# - Produces: target/bundle/Hotki.app

set -euo pipefail
IFS=$'\n\t'

abort() { echo "error: $*" >&2; exit 1; }

# Ensure we're on macOS
[[ "$(uname)" == "Darwin" ]] || abort "This bundler only runs on macOS."

ROOT_DIR="$(cd "$(dirname "$0")/.." &> /dev/null && pwd)"
cd "$ROOT_DIR"

# Configurable via env; sensible defaults otherwise
APP_NAME=${APP_NAME:-Hotki}
BIN_NAME=${BIN_NAME:-hotki}
BUNDLE_ID=${BUNDLE_ID:-si.corte.hotki}
OUT_DIR="$ROOT_DIR/target/bundle"
APP_DIR="$OUT_DIR/${APP_NAME}.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RES_DIR="$CONTENTS_DIR/Resources"
ICONSET_DIR="$OUT_DIR/icon.iconset"
ICNS_FILE="$RES_DIR/${BIN_NAME}.icns"
ICON_SRC="$ROOT_DIR/crates/hotki/assets/logo.png"

command -v sips >/dev/null 2>&1 || abort "sips not found (part of macOS)."
command -v iconutil >/dev/null 2>&1 || abort "iconutil not found (part of macOS Xcode tools)."
command -v cargo >/dev/null 2>&1 || abort "cargo not found. Install Rust toolchain."

[[ -f "$ICON_SRC" ]] || abort "Icon source not found: $ICON_SRC"

# Extract version from the workspace Cargo.toml [workspace.package]
VERSION=$(awk '
  BEGIN { in_section=0 }
  /^\[workspace\.package\]/ { in_section=1; next }
  /^\[/ { in_section=0 }
  in_section && $1=="version" && $2=="=" {
    gsub(/"/, "", $3); print $3; exit
  }
' Cargo.toml)

[[ -n "${VERSION:-}" ]] || abort "Unable to determine version from Cargo.toml"

echo "==> Building release binary ($BIN_NAME $VERSION)"
cargo build --release -p hotki

BIN_PATH="$ROOT_DIR/target/release/$BIN_NAME"
[[ -f "$BIN_PATH" ]] || abort "Built binary not found at $BIN_PATH"

echo "==> Preparing bundle directories"
rm -rf "$APP_DIR" "$ICONSET_DIR"
mkdir -p "$MACOS_DIR" "$RES_DIR" "$ICONSET_DIR"

echo "==> Generating .icns from $ICON_SRC"
# Generate required iconset sizes
declare -a sizes=(
  16 32
  32 64
  128 256
  256 512
  512 1024
)

for ((i=0; i<${#sizes[@]}; i+=2)); do
  base=${sizes[i]}
  two=${sizes[i+1]}
  sips -z "$base" "$base" "$ICON_SRC" --out "$ICONSET_DIR/icon_${base}x${base}.png" >/dev/null
  sips -z "$two" "$two" "$ICON_SRC" --out "$ICONSET_DIR/icon_${base}x${base}@2x.png" >/dev/null
done

iconutil -c icns "$ICONSET_DIR" -o "$ICNS_FILE"

echo "==> Writing Info.plist"
cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>${BIN_NAME}</string>
  <key>CFBundleIconFile</key>
  <string>${BIN_NAME}</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <!-- Hide from Dock and Cmd+Tab  -->
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
PLIST

echo "==> Copying binary"
cp "$BIN_PATH" "$MACOS_DIR/$BIN_NAME"
chmod +x "$MACOS_DIR/$BIN_NAME"

echo "==> Bundle ready: $APP_DIR"
echo "You can run it with: open \"$APP_DIR\""

