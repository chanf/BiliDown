#!/bin/bash
set -e

echo "Testing FFmpeg detection..."

# Test bundled FFmpeg
echo "1. Testing bundled FFmpeg (dev mode):"
if [ -f "src-tauri/resources/bin/ffmpeg" ]; then
    ./src-tauri/resources/bin/ffmpeg -version | head -1
    echo "✓ Bundled FFmpeg works"
else
    echo "✗ Bundled FFmpeg not found"
fi

# Test system FFmpeg
echo -e "\n2. Testing system FFmpeg:"
if command -v ffmpeg &> /dev/null; then
    ffmpeg -version | head -1
    echo "✓ System FFmpeg works"
else
    echo "✗ System FFmpeg not found"
fi

echo -e "\nAll tests completed!"
