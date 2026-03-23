use crate::bilibili::{BilibiliClient, CollectionMode, PlaylistVideo};
use crate::login::{BilibiliLogin, LoginStatus};
use crate::downloader::{
    DownloadConfig, DownloadManager, DownloadState, DownloadTask, StartDownloadRequest,
};
use crate::LoginState;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct ParseUrlResult {
    pub r#type: String,
    pub title: String,
    pub videos: Vec<PlaylistVideo>,
}

#[tauri::command]
pub async fn parse_url(url: String, collection_mode: Option<String>) -> Result<ParseUrlResult, String> {
    let client = BilibiliClient::new();
    let mode = CollectionMode::from_option_str(collection_mode.as_deref());

    let (_url_type, bvid) = BilibiliClient::parse_url(&url)
        .map_err(|e| e.to_string())?;

    let bvid = bvid.ok_or("无法解析视频 ID")?;

    let playlist = client.get_video_playlist_with_mode(&bvid, mode)
        .await
        .map_err(|e| e.to_string())?;

    Ok(ParseUrlResult {
        r#type: playlist.r#type,
        title: playlist.title,
        videos: playlist.videos,
    })
}

#[tauri::command]
pub fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("读取剪切板失败: {}", e))?;
    clipboard
        .get_text()
        .map_err(|e| format!("读取剪切板失败: {}", e))
}

#[tauri::command]
pub async fn get_login_status(state: State<'_, LoginState>) -> Result<LoginStatusResult, String> {
    // 先获取 qrcode_key 并释放锁
    let key = {
        let qrcode_key = state.qrcode_key.lock().unwrap();
        qrcode_key.clone()
    };

    if let Some(key) = key {
        let login = BilibiliLogin::new();
        let status = login.poll_login_status(&key).await;

        match status {
            LoginStatus::Success { url, .. } => {
                // 清空 qrcode_key
                *state.qrcode_key.lock().unwrap() = None;
                // 从登录 URL 中提取 SESSDATA
                if let Ok(extracted) = extract_sessdata_from_url(&url).await {
                    *state.sessdata.lock().unwrap() = Some(extracted.clone());
                    // 保存到本地文件
                    state.save_sessdata(&extracted);
                } else {
                    eprintln!("登录成功，但未能从回调 URL 提取 SESSDATA，缓存可能不会持久化");
                }
                Ok(LoginStatusResult {
                    logged_in: true,
                    username: None,
                })
            }
            _ => Ok(LoginStatusResult {
                logged_in: state.sessdata.lock().unwrap().is_some(),
                username: None,
            }),
        }
    } else {
        Ok(LoginStatusResult {
            logged_in: state.sessdata.lock().unwrap().is_some(),
            username: None,
        })
    }
}

#[tauri::command]
pub fn logout(state: State<'_, LoginState>) -> Result<(), String> {
    // 清除内存中的 sessdata
    *state.sessdata.lock().unwrap() = None;
    // 清除本地文件
    state.clear_sessdata();
    Ok(())
}

/// 从 B 站登录成功 URL 中提取 SESSDATA
async fn extract_sessdata_from_url(url: &str) -> Result<String, String> {
    // B站登录成功后，SESSDATA 通常已经在浏览器端设置
    // 这里的 URL 是重定向 URL，我们需要从它中解析或访问它来获取 cookies

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    // 从响应头中提取 SESSDATA
    let cookies = response.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>();

    for cookie in cookies {
        if cookie.starts_with("SESSDATA=") || cookie.starts_with("sessdata=") {
            if let Some(sessdata) = cookie.split(';').next() {
                let sessdata = sessdata
                    .trim_start_matches("SESSDATA=")
                    .trim_start_matches("sessdata=");
                return Ok(sessdata.to_string());
            }
        }
    }

    // 如果响应头中没有，尝试从 URL（含多层编码）中提取
    if let Some(sessdata) = find_sessdata_in_text(url) {
        return Ok(sessdata);
    }

    let mut decoded = url.to_string();
    for _ in 0..2 {
        let next = match urlencoding::decode(&decoded) {
            Ok(v) => v.into_owned(),
            Err(_) => break,
        };
        if next == decoded {
            break;
        }

        if let Some(sessdata) = find_sessdata_in_text(&next) {
            return Ok(sessdata);
        }
        decoded = next;
    }

    Err("无法提取 SESSDATA".to_string())
}

fn find_sessdata_in_text(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?i)(?:[?&]|^)sessdata=([^&#]+)").ok()?;
    re.captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginStatusResult {
    pub logged_in: bool,
    pub username: Option<String>,
}

#[tauri::command]
pub async fn get_qrcode(state: State<'_, LoginState>) -> Result<QrcodeResult, String> {
    let login = BilibiliLogin::new();
    let result = login.get_qrcode()
        .await
        .map_err(|e| e.to_string())?;

    *state.qrcode_key.lock().unwrap() = Some(result.qrcode_key.clone());

    Ok(QrcodeResult {
        url: result.url,
        qrcode_image: result.qrcode_image,
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QrcodeResult {
    pub url: String,
    pub qrcode_image: String,
}

#[derive(Debug, Deserialize)]
pub struct VideoToDownload {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
}

#[tauri::command]
pub async fn download(
    videos: Vec<VideoToDownload>,
    save_path: Option<String>,
    download_state: State<'_, DownloadState>,
    login_state: State<'_, LoginState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    if videos.is_empty() {
        return Err("下载列表为空".to_string());
    }

    if let Some(path) = save_path {
        let mut config = download_state.config.lock().unwrap();
        config.save_path = path;
    }

    let config = download_state.config.lock().unwrap().clone();
    let sessdata = login_state.sessdata.lock().unwrap().clone();

    let mut client = BilibiliClient::new();
    if let Some(sessdata) = sessdata {
        client = client.with_sessdata(sessdata);
    }

    let manager = DownloadManager::new(&download_state, app);
    let mut task_ids = Vec::new();

    for video in videos {
        let play_url = client.get_play_url(&video.bvid, video.cid, config.quality)
            .await
            .map_err(|e| format!("获取播放URL失败: {}", e))?;

        let request = StartDownloadRequest {
            bvid: video.bvid.clone(),
            cid: video.cid,
            title: video.title.clone(),
            part_title: video.part_title.clone(),
            video_url: play_url.video_url,
            audio_url: play_url.audio_url,
            config: config.clone(),
        };

        let task_id = manager
            .create_and_start(request)
            .await
            .map_err(|e| format!("创建下载任务失败: {}", e))?;

        task_ids.push(task_id);
    }

    Ok(format!("已添加 {} 个下载任务", task_ids.len()))
}

#[tauri::command]
pub async fn pause_download(
    task_id: String,
    state: State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let manager = DownloadManager::new(&state, app);
    manager
        .pause_task(&task_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resume_download(
    task_id: String,
    state: State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let manager = DownloadManager::new(&state, app);
    manager
        .resume_task(&task_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_download(
    task_id: String,
    clean_files: bool,
    state: State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let manager = DownloadManager::new(&state, app);
    manager
        .delete_task(&task_id, clean_files)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_download_progress(state: State<'_, DownloadState>) -> Vec<DownloadTask> {
    state
        .tasks
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect()
}

#[tauri::command]
pub fn get_download_config(state: State<'_, DownloadState>) -> DownloadConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
pub async fn set_download_config(
    config: DownloadConfig,
    state: State<'_, DownloadState>,
) -> Result<(), String> {
    *state.config.lock().unwrap() = config;
    Ok(())
}

#[tauri::command]
pub fn open_download_dir(state: State<'_, DownloadState>) -> Result<(), String> {
    let config = state.config.lock().unwrap();
    let path = &config.save_path;

    // 如果路径为空，使用默认目录
    let path_to_open = if path.is_empty() {
        dirs::video_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap())
    } else {
        std::path::PathBuf::from(path)
    };

    // 使用系统命令打开目录
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path_to_open)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path_to_open)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&path_to_open)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
