use crate::bilibili::{BilibiliClient, PlaylistResult, PlaylistVideo};
use crate::login::{BilibiliLogin, LoginStatus};
use crate::downloader::{DownloadManager, DownloadState, DownloadTask, DownloadConfig, TaskStatus, ResumeManager, ResumeMetadata};
use crate::LoginState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{Emitter, State};

#[derive(Debug, Serialize, Deserialize)]
pub struct ParseUrlResult {
    pub r#type: String,
    pub title: String,
    pub videos: Vec<PlaylistVideo>,
}

#[tauri::command]
pub async fn parse_url(url: String) -> Result<ParseUrlResult, String> {
    let client = BilibiliClient::new();

    let (_url_type, bvid) = BilibiliClient::parse_url(&url)
        .map_err(|e| e.to_string())?;

    let bvid = bvid.ok_or("无法解析视频 ID")?;

    let playlist = client.get_video_playlist(&bvid)
        .await
        .map_err(|e| e.to_string())?;

    Ok(ParseUrlResult {
        r#type: playlist.r#type,
        title: playlist.title,
        videos: playlist.videos,
    })
}

#[tauri::command]
pub fn get_login_status(state: State<'_, LoginState>) -> Result<LoginStatusResult, String> {
    let mut qrcode_key = state.qrcode_key.lock().unwrap();

    if let Some(key) = qrcode_key.as_ref() {
        let login = BilibiliLogin::new();
        let status = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(login.poll_login_status(key));

        match status {
            LoginStatus::Success { .. } => {
                *qrcode_key = None;
                Ok(LoginStatusResult {
                    logged_in: true,
                    username: None,
                })
            }
            _ => Ok(LoginStatusResult {
                logged_in: false,
                username: None,
            }),
        }
    } else {
        Ok(LoginStatusResult {
            logged_in: false,
            username: None,
        })
    }
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
    state: State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    if let Some(path) = save_path {
        let mut config = state.config.lock().unwrap();
        config.save_path = path;
    }

    let config = state.config.lock().unwrap().clone();
    let client = BilibiliClient::new();

    let cache_dir = dirs::cache_dir()
        .unwrap()
        .join("bilibili-downloader");
    let resume_manager = ResumeManager::new(&cache_dir);

    let mut task_ids = Vec::new();

    for video in videos {
        let play_url = client.get_play_url(&video.bvid, video.cid, config.quality)
            .await
            .map_err(|e| format!("获取播放URL失败: {}", e))?;

        let task_id = resume_manager.generate_task_id(&video.bvid, video.cid);

        let metadata = ResumeMetadata {
            task_id: task_id.clone(),
            bvid: video.bvid.clone(),
            cid: video.cid,
            title: video.title.clone(),
            part_title: video.part_title.clone(),
            video_url: play_url.video_url,
            audio_url: play_url.audio_url,
            video_size: play_url.video_size,
            audio_size: play_url.audio_size,
            video_chunks: vec![],
            audio_chunks: vec![],
            config: config.clone(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };

        resume_manager.save_metadata(&metadata).await
            .map_err(|e| format!("保存元数据失败: {}", e))?;

        let task = DownloadTask {
            task_id: task_id.clone(),
            bvid: video.bvid.clone(),
            cid: video.cid,
            title: video.title.clone(),
            part_title: video.part_title.clone(),
            status: TaskStatus::Pending,
            video_progress: 0.0,
            audio_progress: 0.0,
            video_size: play_url.video_size,
            audio_size: play_url.audio_size,
            video_downloaded: 0,
            audio_downloaded: 0,
            speed: 0,
            save_path: config.save_path.clone(),
            filename: format!("{}.mp4", video.title),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };

        state.tasks.lock().unwrap().insert(task_id.clone(), task);
        app.emit("download-progress", &state.tasks.lock().unwrap().get(&task_id).unwrap())
            .map_err(|e| format!("发送事件失败: {}", e))?;

        let resume_manager_clone = ResumeManager::new(&cache_dir);
        let task_id_for_runner = task_id.clone();
        let app_clone = app.clone();

        tokio::spawn(async move {
            use crate::downloader::task::DownloadTaskRunner;

            let ffmpeg_path = crate::ffmpeg::FFmpegDetector::new(
                dirs::data_dir().unwrap().join("bilibili-downloader")
            ).get_or_install_ffmpeg().await.unwrap_or_else(|_| "ffmpeg".to_string());

            let runner = DownloadTaskRunner::new(
                task_id_for_runner,
                metadata,
                resume_manager_clone,
                ffmpeg_path,
                app_clone,
            );

            if let Err(e) = runner.run().await {
                eprintln!("下载任务失败: {}", e);
            }
        });

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
