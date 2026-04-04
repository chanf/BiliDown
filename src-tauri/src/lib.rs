mod bilibili;
mod login;
mod commands;
mod downloader;
mod ffmpeg;
mod logger;

use commands::*;
use downloader::DownloadState;
use std::sync::Mutex;
use std::path::PathBuf;

// 全局登录状态
struct LoginState {
    qrcode_key: Mutex<Option<String>>,
    sessdata: Mutex<Option<String>>,
    config_dir: PathBuf,
}

impl LoginState {
    /// 从本地文件加载已保存的 SESSDATA
    fn load_sessdata(&self) -> Option<String> {
        let sessdata_file = self.config_dir.join("sessdata.txt");
        if let Ok(content) = std::fs::read_to_string(&sessdata_file) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    }

    /// 保存 SESSDATA 到本地文件
    fn save_sessdata(&self, sessdata: &str) {
        let sessdata_file = self.config_dir.join("sessdata.txt");
        let _ = std::fs::create_dir_all(&self.config_dir);
        let _ = std::fs::write(&sessdata_file, sessdata);
    }

    /// 清除已保存的 SESSDATA
    fn clear_sessdata(&self) {
        let sessdata_file = self.config_dir.join("sessdata.txt");
        let _ = std::fs::remove_file(&sessdata_file);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 获取配置目录
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap())
        .join("bilibili-downloader");

    // 初始化下载状态（从文件加载配置）
    let download_state = DownloadState {
        tasks: Mutex::new(std::collections::HashMap::new()),
        active_tasks: Mutex::new(std::collections::HashMap::new()),
        controls: Mutex::new(std::collections::HashMap::new()),
        config: Mutex::new(downloader::DownloadConfig::load_from_file()),
    };

    // 初始化登录状态
    let login_state = LoginState {
        qrcode_key: Mutex::new(None),
        sessdata: Mutex::new(None),
        config_dir: config_dir.clone(),
    };

    // 从本地加载已保存的 SESSDATA
    if let Some(saved_sessdata) = login_state.load_sessdata() {
        *login_state.sessdata.lock().unwrap() = Some(saved_sessdata);
        eprintln!("已加载保存的登录凭证");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // 初始化日志系统
            logger::init(app.handle().clone());
            Ok(())
        })
        .manage(login_state)
        .manage(download_state)
        .invoke_handler(tauri::generate_handler![
            parse_url,
            read_clipboard_text,
            get_login_status,
            get_qrcode,
            download,
            pause_download,
            resume_download,
            delete_download,
            get_download_progress,
            get_download_config,
            set_download_config,
            select_download_folder,
            open_download_dir,
            logout,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
