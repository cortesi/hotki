#!/usr/bin/env bash

# Resize the orange dev logo to create a dev tray icon

set -euo pipefail
sips -z 22 22 crates/hotki-app/assets/logo-dev.png --out crates/hotki-app/assets/tray-icon-dev.png
