#!/bin/bash
# 检查应用日志中的 FFmpeg 使用情况

echo "检查应用日志中的 FFmpeg 信息..."
echo ""

# 如果应用正在运行，检查其输出
if pgrep -f "bilibili-downloader" > /dev/null; then
    echo "✓ 应用正在运行"
    echo "  PID: $(pgrep -f 'target/debug/bilibili-downloader')"
else
    echo "✗ 应用未运行"
    echo "  启动命令: ./scripts/restart-tauri-dev.sh"
fi

echo ""
echo "内置 FFmpeg 验证:"
if [ -f "src-tauri/resources/bin/ffmpeg" ]; then
    echo "✓ 内置 FFmpeg 存在"
    echo "  路径: src-tauri/resources/bin/ffmpeg"
    echo "  版本: $(src-tauri/resources/bin/ffmpeg -version | head -1)"
else
    echo "✗ 内置 FFmpeg 不存在"
fi
