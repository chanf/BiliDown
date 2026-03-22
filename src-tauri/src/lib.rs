mod bilibili;
mod login;
mod commands;
mod downloader;
mod ffmpeg;

use commands::*;
use downloader::DownloadState;
use std::sync::Mutex;

// 全局登录状态
struct LoginState {
    qrcode_key: Mutex<Option<String>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化下载状态
    let download_state = DownloadState {
        tasks: Mutex::new(std::collections::HashMap::new()),
        active_tasks: Mutex::new(std::collections::HashMap::new()),
        config: Mutex::new(downloader::DownloadConfig::default()),
        ffmpeg_path: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(LoginState {
            qrcode_key: Mutex::new(None),
        })
        .manage(download_state)
        .invoke_handler(tauri::generate_handler![
            parse_url,
            get_login_status,
            get_qrcode,
            download,
            pause_download,
            resume_download,
            delete_download,
            get_download_progress,
            get_download_config,
            set_download_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
