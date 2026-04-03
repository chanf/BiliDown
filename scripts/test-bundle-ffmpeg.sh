#!/bin/bash

set -e

DMG_PATH="src-tauri/target/release/bundle/dmg/bilibili-downloader_0.1.0_aarch64.dmg"
MOUNT_POINT="/tmp/bilidown-test-mount"

echo "🔍 测试打包后的 FFmpeg 检测"
echo "================================"

# 挂载 DMG
echo "📦 挂载 DMG..."
hdiutil attach "$DMG_PATH" -readonly -nobrowse -mountpoint "$MOUNT_POINT" 2>&1 | grep -v "CRC32\|verified\|disk image" || true

APP_PATH="$MOUNT_POINT/bilibili-downloader.app"
EXE_PATH="$APP_PATH/Contents/MacOS/bilibili-downloader"

echo "📁 App 路径: $APP_PATH"
echo ""

# 检查 FFmpeg 文件
echo "🔎 检查 FFmpeg 文件位置:"
FFMPEG_PATH=$(find "$APP_PATH" -name "ffmpeg" -type f)
if [ -n "$FFMPEG_PATH" ]; then
    echo "✓ 找到 FFmpeg: $FFMPEG_PATH"
    # 检查是否可执行
    if [ -x "$FFMPEG_PATH" ]; then
        echo "✓ FFmpeg 可执行权限正确"
        # 测试运行
        VERSION=$("$FFMPEG_PATH" -version 2>&1 | head -1)
        echo "✓ FFmpeg 版本: $VERSION"
    else
        echo "✗ FFmpeg 缺少可执行权限"
    fi
else
    echo "✗ 未找到 FFmpeg"
fi

echo ""
echo "📂 App 目录结构:"
ls -la "$APP_PATH/Contents/"
echo ""
echo "📂 Resources 目录结构:"
ls -laR "$APP_PATH/Contents/Resources/" | head -20

echo ""
echo "🧪 模拟路径检测逻辑:"

# 模拟检测逻辑
if [ -f "$EXE_PATH" ]; then
    EXE_DIR=$(dirname "$EXE_PATH")
    CONTENTS_DIR=$(dirname "$EXE_DIR")
    RESOURCES_DIR="$CONTENTS_DIR/Resources"

    echo "  可执行文件: $EXE_PATH"
    echo "  MacOS 目录: $EXE_DIR"
    echo "  Contents 目录: $CONTENTS_DIR"
    echo "  Resources 目录: $RESOURCES_DIR"
    echo ""

    # 测试路径 1: Resources/bin/ffmpeg
    TEST_PATH_1="$RESOURCES_DIR/bin/ffmpeg"
    echo "  测试路径 1: $TEST_PATH_1"
    if [ -f "$TEST_PATH_1" ]; then
        echo "  ✓ 路径 1 存在"
    else
        echo "  ✗ 路径 1 不存在"
    fi

    # 测试路径 2: Resources/resources/bin/ffmpeg
    TEST_PATH_2="$RESOURCES_DIR/resources/bin/ffmpeg"
    echo "  测试路径 2: $TEST_PATH_2"
    if [ -f "$TEST_PATH_2" ]; then
        echo "  ✓ 路径 2 存在"
    else
        echo "  ✗ 路径 2 不存在"
    fi
fi

# 卸载 DMG
echo ""
echo "📤 卸载 DMG..."
hdiutil detach "$MOUNT_POINT" 2>&1 | grep -v "CRC32\|verified\|disk image" || true

echo ""
echo "✅ 测试完成"
