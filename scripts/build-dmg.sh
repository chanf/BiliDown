#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUNDLE_DIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/dmg"

open_after_build=false
if [[ "${1:-}" == "--open" ]]; then
  open_after_build=true
fi

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "[DiliDown] 缺少命令: ${cmd}"
    exit 1
  fi
}

require_cmd npm

echo "[DiliDown] 项目目录: ${PROJECT_ROOT}"
echo "[DiliDown] 开始构建 DMG..."

cd "${PROJECT_ROOT}"
npm run tauri build -- --bundles dmg

if [[ ! -d "${BUNDLE_DIR}" ]]; then
  echo "[DiliDown] 未找到 DMG 输出目录: ${BUNDLE_DIR}"
  exit 1
fi

latest_dmg="$(ls -t "${BUNDLE_DIR}"/*.dmg 2>/dev/null | head -n 1 || true)"
if [[ -z "${latest_dmg}" ]]; then
  echo "[DiliDown] 构建完成，但未找到 DMG 文件"
  exit 1
fi

echo "[DiliDown] DMG 构建成功"
echo "[DiliDown] 输出文件: ${latest_dmg}"

if ${open_after_build}; then
  echo "[DiliDown] 在 Finder 中定位 DMG"
  open -R "${latest_dmg}"
fi
