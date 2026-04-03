# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Common Development Commands

```bash
# Start development environment (auto-kills old processes)
./scripts/restart-tauri-dev.sh

# Manual dev start (if script fails)
npm run tauri dev

# Frontend build check
npm run build

# Build macOS DMG
./scripts/build-dmg.sh
./scripts/build-dmg.sh --open  # Open Finder to DMG location

# Clean build artifacts (preserves node_modules)
./scripts/clean-build.sh
```

## Architecture Overview

**BiliDown** is a Tauri 2.x desktop app for downloading Bilibili videos. The architecture follows a clear separation between frontend (React) and backend (Rust).

### Technology Stack
- **Frontend**: React 19 + TypeScript + Vite
- **Backend**: Rust + Tauri 2.x + Tokio
- **Video Processing**: FFmpeg (auto-detected/installed, falls back to system `ffmpeg`)

### Backend Module Structure

```
src-tauri/src/
├── lib.rs              # App entry point, manages global state
├── commands.rs         # Tauri command handlers (parse_url, download, etc.)
├── bilibili.rs         # Bilibili API client & collection detection
├── login.rs            # QR code login & session polling
├── downloader/         # Download pipeline (refactored 2026-03-23)
│   ├── manager.rs      # Task lifecycle & event emission
│   ├── chunked.rs      # Sequential range download with .part resume
│   └── merger.rs       # FFmpeg audio/video merging
└── ffmpeg/             # FFmpeg detection & installation
    └── detector.rs
```

### Key Design Patterns

**Task State Machine**: `Pending -> Downloading -> Paused -> Merging -> Completed/Failed`
- Tasks controlled in-memory via `TaskControl` (AtomicBool paused/cancelled)
- Progress events emitted via Tauri's `emit()` to frontend
- No cross-restart persistence yet (planned for v0.3)

**Download Pipeline** (simplified for stability):
1. Sequential chunked download to `.part` files (not parallel)
2. FFmpeg merges video/audio streams to final `.mp4`
3. Temp files cleaned from `~/.cache/bilibili-downloader/tasks/{task_id}/`

**Collection Detection Priority**:
1. Multi-part video (pages.len > 1)
2. `ugc_season` structured data (current section only)
3. (Compat mode only) HTML `__INITIAL_STATE__` fallback
4. Single video fallback

Avoids aggressive "scan all page links" approach that误判 UP 主全部视频.

### Critical Invariants

- **Frontend-Backend Contract**: Tauri commands must remain stable for frontend compatibility
- **SESSDATA Persistence**: Login state cached at `~/.config/bilibili-downloader/sessdata.txt`, loaded on startup
- **Download Defaults**: Save path defaults to `~/Movies/DiliDown`, quality to `80` (1080P)
- **Filename Sanitization**: Invalid path chars replaced with `_`, defaults to "未命名视频" if empty

### Download Configuration

Config accessed via `DownloadState.config: Mutex<DownloadConfig>`:
- `save_path`: Default `~/Movies/DiliDown`
- `concurrent_connections`: HTTP concurrent requests (default 4)
- `chunk_size`: Download chunk size in bytes (default 1MB)
- `quality`: Bilibili qn value (80=1080P, 64=720P, etc.)
- `max_retry`: Retry attempts per chunk (default 3)
- `timeout`: Per-request timeout in seconds (default 30)

### Adding New Tauri Commands

1. Define command in `src-tauri/src/commands.rs` with `#[tauri::command]`
2. Register in `lib.rs` `invoke_handler!` macro
3. Frontend calls via `invoke()` from `@tauri-apps/api/core`
4. For progress updates, emit events via `app_handle.emit("event-name", data)`

### Known Limitations

- Only supports `video/BV...` URLs (not playlists, bangumi, etc.)
- Tasks do not persist across app restarts
- Some content restricted by platform risk control, membership, or region
