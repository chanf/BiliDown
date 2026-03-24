#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

REMOVED_COUNT=0
SKIPPED_COUNT=0

resolve_abs_path() {
  local target_path="$1"
  local target_dir
  local target_base
  target_dir="$(cd "$(dirname "${target_path}")" && pwd -P)"
  target_base="$(basename "${target_path}")"
  printf '%s/%s\n' "${target_dir}" "${target_base}"
}

remove_dir_if_exists() {
  local relative_path="$1"
  local target_path="${PROJECT_ROOT}/${relative_path}"

  if [[ ! -e "${target_path}" ]]; then
    echo "[DiliDown] 跳过（不存在）: ${relative_path}"
    SKIPPED_COUNT=$((SKIPPED_COUNT + 1))
    return
  fi

  if [[ ! -d "${target_path}" ]]; then
    echo "[DiliDown] 跳过（非目录）: ${relative_path}"
    SKIPPED_COUNT=$((SKIPPED_COUNT + 1))
    return
  fi

  local abs_target
  abs_target="$(resolve_abs_path "${target_path}")"

  case "${abs_target}" in
    "${PROJECT_ROOT}"/*)
      ;;
    *)
      echo "[DiliDown] 安全检查失败，拒绝删除: ${abs_target}"
      exit 1
      ;;
  esac

  if [[ "${abs_target}" == "${PROJECT_ROOT}" ]]; then
    echo "[DiliDown] 安全检查失败，拒绝删除项目根目录"
    exit 1
  fi

  rm -rf "${abs_target}"
  echo "[DiliDown] 已删除: ${relative_path}"
  REMOVED_COUNT=$((REMOVED_COUNT + 1))
}

echo "[DiliDown] 项目目录: ${PROJECT_ROOT}"
echo "[DiliDown] 开始清理构建过程文件..."

remove_dir_if_exists "dist"
remove_dir_if_exists "src-tauri/target"
remove_dir_if_exists "src-tauri/gen"

echo "[DiliDown] 清理完成: 已删除 ${REMOVED_COUNT} 项，跳过 ${SKIPPED_COUNT} 项"
