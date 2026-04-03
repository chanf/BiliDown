#!/bin/bash
set -e

echo "Testing FFmpeg detection in BiliDown..."

# Test 1: Verify bundled FFmpeg exists
echo "1. Checking bundled FFmpeg:"
if [ -f "src-tauri/resources/bin/ffmpeg" ]; then
    echo "✓ Bundled FFmpeg found at src-tauri/resources/bin/ffmpeg"
    ./src-tauri/resources/bin/ffmpeg -version | head -1
else
    echo "✗ Bundled FFmpeg not found"
    exit 1
fi

# Test 2: Verify system FFmpeg
echo -e "\n2. Checking system FFmpeg:"
if command -v ffmpeg &> /dev/null; then
    echo "✓ System FFmpeg found"
    ffmpeg -version | head -1
else
    echo "✗ System FFmpeg not found"
fi

# Test 3: Check app directory structure
echo -e "\n3. Checking app directory structure:"
echo "Project root: $(pwd)"
echo "Resources dir: $(ls -la src-tauri/resources/bin/ 2>/dev/null || echo 'Not found')"

echo -e "\n✓ All checks passed!"
