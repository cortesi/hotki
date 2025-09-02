#!/usr/bin/env bash

# Build a macOS .app bundle for hotki development builds.
# - Builds debug binary
# - Generates .icns from the orange dev logo
# - Writes Info.plist
# - Produces: target/bundle-dev/Hotki-Dev.app

set -euo pipefail
IFS=$'\n\t'

abort() { echo "error: $*" >&2; exit 1; }

# Ensure we're on macOS
[[ "$(uname)" == "Darwin" ]] || abort "This bundler only runs on macOS."

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# Configurable via env; sensible defaults otherwise
APP_NAME=${APP_NAME:-Hotki-Dev}
BIN_NAME=${BIN_NAME:-hotki}
BUNDLE_ID=${BUNDLE_ID:-si.corte.hotki.dev}
OUT_DIR="$ROOT_DIR/target/bundle-dev"
APP_DIR="$OUT_DIR/${APP_NAME}.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RES_DIR="$CONTENTS_DIR/Resources"
ICONSET_DIR="$OUT_DIR/icon.iconset"
ICNS_FILE="$RES_DIR/${BIN_NAME}.icns"
ICON_SRC="$ROOT_DIR/crates/hotki/assets/logo-dev.png"

command -v sips >/dev/null 2>&1 || abort "sips not found (part of macOS)."
command -v iconutil >/dev/null 2>&1 || abort "iconutil not found (part of macOS Xcode tools)."
command -v cargo >/dev/null 2>&1 || abort "cargo not found. Install Rust toolchain."

[[ -f "$ICON_SRC" ]] || abort "Icon source not found: $ICON_SRC"

# Extract version from the workspace Cargo.toml [workspace.package]
VERSION=$(awk '
  /^\[workspace\.package\]/ { in_section=1 }
  in_section && /^version\s*=/ {
    match($0, /"([^"]+)"/, arr)
    print arr[1]
    exit
  }
  /^\[/ && !/^\[workspace\.package\]/ { in_section=0 }
' Cargo.toml)

[[ -n "$VERSION" ]] || abort "Could not extract version from Cargo.toml"

echo "Building ${APP_NAME} v${VERSION} (debug build)..."

# Build debug binary
echo "Building debug binary..."
cargo build --bin "$BIN_NAME"
BINARY_PATH="$ROOT_DIR/target/debug/$BIN_NAME"
[[ -f "$BINARY_PATH" ]] || abort "Binary not found at $BINARY_PATH"

# Create bundle structure
echo "Creating app bundle structure..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RES_DIR"

# Copy binary
cp "$BINARY_PATH" "$MACOS_DIR/$BIN_NAME"

# Generate .icns from orange dev logo
echo "Generating .icns from dev logo..."
rm -rf "$ICONSET_DIR"
mkdir -p "$ICONSET_DIR"

# Generate various icon sizes
for size in 16 32 64 128 256 512; do
  sips -z $size $size "$ICON_SRC" --out "$ICONSET_DIR/icon_${size}x${size}.png" >/dev/null 2>&1
  size2=$((size * 2))
  sips -z $size2 $size2 "$ICON_SRC" --out "$ICONSET_DIR/icon_${size}x${size}@2x.png" >/dev/null 2>&1
done

iconutil -c icns -o "$ICNS_FILE" "$ICONSET_DIR"
rm -rf "$ICONSET_DIR"

# Write Info.plist
cat > "$CONTENTS_DIR/Info.plist" << EOF
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
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

echo "✅ Dev bundle created: $APP_DIR"
echo "   Orange icons indicate this is a development build"
echo ""
echo "To run: open '$APP_DIR'"
echo "To install: cp -r '$APP_DIR' /Applications/"