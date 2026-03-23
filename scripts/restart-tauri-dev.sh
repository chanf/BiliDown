#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

collect_pids() {
  ps -eo pid=,command= | while IFS= read -r line; do
    pid="$(printf '%s\n' "${line}" | awk '{print $1}')"
    cmd="${line#${pid}}"

    case "${cmd}" in
      *"${PROJECT_ROOT}"*)
        ;;
      *)
        continue
        ;;
    esac

    if [[ "${cmd}" == *"npm run tauri dev"* ]] \
      || [[ "${cmd}" == *"tauri dev"* ]] \
      || [[ "${cmd}" == *"npm run dev"* ]] \
      || [[ "${cmd}" == *"vite"* ]] \
      || [[ "${cmd}" == *"cargo  run --no-default-features"* ]] \
      || [[ "${cmd}" == *"cargo run --no-default-features"* ]] \
      || [[ "${cmd}" == *"target/debug/bilibili-downloader"* ]]; then
      printf '%s\n' "${pid}"
    fi
  done | sort -u
}

echo "[DiliDown] 项目目录: ${PROJECT_ROOT}"

pids=()
while IFS= read -r pid; do
  [[ -n "${pid}" ]] && pids+=("${pid}")
done < <(collect_pids)

if (( ${#pids[@]} > 0 )); then
  echo "[DiliDown] 检测到运行中的进程: ${pids[*]}"
  kill "${pids[@]}" 2>/dev/null || true
  sleep 1

  remaining=()
  for pid in "${pids[@]}"; do
    if kill -0 "${pid}" 2>/dev/null; then
      remaining+=("${pid}")
    fi
  done
  if (( ${#remaining[@]} > 0 )); then
    echo "[DiliDown] 仍在运行，执行强制结束: ${remaining[*]}"
    kill -9 "${remaining[@]}" 2>/dev/null || true
  fi
else
  echo "[DiliDown] 未检测到运行中的开发进程"
fi

cd "${PROJECT_ROOT}"
echo "[DiliDown] 启动客户端: npm run tauri dev"
exec npm run tauri dev
