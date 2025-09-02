#!/bin/bash
# Generate documentation logo with dark gray background from transparent master logo

# Check if ImageMagick is installed
if ! command -v convert &> /dev/null; then
    echo "ImageMagick is required but not installed."
    echo "Install with: brew install imagemagick"
    exit 1
fi

# Define paths
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
MASTER_LOGO="$PROJECT_ROOT/crates/hotki/assets/logo.png"
DOC_LOGO="$PROJECT_ROOT/crates/hotki/assets/logo-doc.png"

# Check if master logo exists
if [ ! -f "$MASTER_LOGO" ]; then
    echo "Master logo not found at: $MASTER_LOGO"
    exit 1
fi

# Generate doc logo with dark gray background (#2d2d2d)
echo "Generating documentation logo..."
convert "$MASTER_LOGO" -background "#2d2d2d" -flatten "$DOC_LOGO"

if [ $? -eq 0 ]; then
    echo "Successfully generated: $DOC_LOGO"
else
    echo "Failed to generate documentation logo"
    exit 1
fi