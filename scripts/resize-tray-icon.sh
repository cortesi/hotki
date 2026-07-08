#!/bin/bash
# Resize tray icon from logo
set -e

cd "$(dirname "$0")/.."
sips -z 22 22 crates/hotki-app/assets/logo.png --out crates/hotki-app/assets/tray-icon.png
