#!/usr/bin/env bash

# Resize the orange dev logo to create a dev tray icon

set -euo pipefail
sips -z 22 22 crates/hotki/assets/logo-dev.png --out crates/hotki/assets/tray-icon-dev.png