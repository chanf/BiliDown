use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};

use crate::downloader::{
    ChunkedDownloader, DownloadConfig, DownloadState, DownloadTask, TaskControl, TaskStatus,
    VideoMerger,
};
use crate::ffmpeg::FFmpegDetector;
use crate::persistence;
use crate::history::{self, HistoryEntry};

#[derive(Debug, Clone)]
pub struct StartDownloadRequest {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub video_url: String,
    pub audio_url: String,
    pub config: DownloadConfig,
    pub collection_type: Option<String>,  // 合集类型: "single", "multi_part", "collection"
    pub collection_title: Option<String>, // 合集标题
}

/// 下载任务管理器
pub struct DownloadManager<'a> {
    state: &'a State<'a, DownloadState>,
    app_handle: tauri::AppHandle,
}

impl<'a> DownloadManager<'a> {
    pub fn new(state: &'a State<'a, DownloadState>, app_handle: tauri::AppHandle) -> Self {
        Self { state, app_handle }
    }

    pub async fn create_and_start(&self, request: StartDownloadRequest) -> Result<String> {
        let task_id = format!("{}_{}_{}", request.bvid, request.cid, uuid::Uuid::new_v4());
        let filename = build_filename(&request.title, request.part_title.as_deref());

        // 根据合集类型计算保存路径
        let save_path = compute_save_path(&request.config.save_path, &request.collection_type, &request.collection_title);

        let now = current_timestamp();

        let task = DownloadTask {
            task_id: task_id.clone(),
            bvid: request.bvid.clone(),
            cid: request.cid,
            title: request.title.clone(),
            part_title: request.part_title.clone(),
            status: TaskStatus::Pending,
            video_progress: 0.0,
            audio_progress: 0.0,
            video_size: 0,
            audio_size: 0,
            video_downloaded: 0,
            audio_downloaded: 0,
            speed: 0,
            save_path,
            filename,
            created_at: now,
            updated_at: now,
            last_speed_update_time: None,
            last_speed_downloaded: 0,
        };

        self.state.tasks.lock().unwrap().insert(task_id.clone(), task);
        let control = TaskControl::new();
        self.state
            .controls
            .lock()
            .unwrap()
            .insert(task_id.clone(), control.clone());
        self.emit_progress(&task_id).await?;

        // 保存任务到文件
        let tasks = self.state.tasks.lock().unwrap();
        let _ = persistence::save_tasks(&tasks);
        drop(tasks);

        let app_handle = self.app_handle.clone();
        let task_id_clone = task_id.clone();
        let request_clone = request.clone();

        let handle = tokio::spawn(async move {
            let state = app_handle.state::<DownloadState>();
            let result = run_download_pipeline(
                &app_handle,
                &state,
                &task_id_clone,
                request_clone,
                control.clone(),
            )
            .await;

            if let Err(err) = result {
                if !err.to_string().contains("任务已取消") {
                    update_task_and_emit(&state, &app_handle, &task_id_clone, |task| {
                        task.status = TaskStatus::Failed(err.to_string());
                    }, true); // 保存失败状态
                }
            }

            state.active_tasks.lock().unwrap().remove(&task_id_clone);
            state.controls.lock().unwrap().remove(&task_id_clone);
        });

        self.state
            .active_tasks
            .lock()
            .unwrap()
            .insert(task_id.clone(), handle);

        Ok(task_id)
    }

    pub async fn pause_task(&self, task_id: &str) -> Result<()> {
        let control = self
            .state
            .controls
            .lock()
            .unwrap()
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("任务不存在: {}", task_id))?;
        control.paused.store(true, Ordering::SeqCst);

        update_task_and_emit(self.state, &self.app_handle, task_id, |task| {
            task.status = TaskStatus::Paused;
        }, false);
        Ok(())
    }

    pub async fn resume_task(&self, task_id: &str) -> Result<()> {
        let control = self
            .state
            .controls
            .lock()
            .unwrap()
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("任务不存在: {}", task_id))?;
        control.paused.store(false, Ordering::SeqCst);

        update_task_and_emit(self.state, &self.app_handle, task_id, |task| {
            if task.status != TaskStatus::Completed {
                task.status = TaskStatus::Downloading;
            }
        }, false);
        Ok(())
    }

    pub async fn delete_task(&self, task_id: &str, clean_files: bool) -> Result<()> {
        if let Some(control) = self.state.controls.lock().unwrap().get(task_id).cloned() {
            control.cancelled.store(true, Ordering::SeqCst);
            control.paused.store(false, Ordering::SeqCst);
        }

        if let Some(handle) = self.state.active_tasks.lock().unwrap().remove(task_id) {
            handle.abort();
        }

        let task = self.state.tasks.lock().unwrap().remove(task_id);
        self.state.controls.lock().unwrap().remove(task_id);

        if clean_files {
            let temp_dir = task_temp_dir(task_id);
            tokio::fs::remove_dir_all(&temp_dir).await.ok();

            if let Some(task) = task {
                let output_path = PathBuf::from(task.save_path).join(task.filename);
                tokio::fs::remove_file(output_path).await.ok();
            }
        }

        // 保存任务到文件
        let tasks = self.state.tasks.lock().unwrap();
        let _ = persistence::save_tasks(&tasks);
        drop(tasks);

        Ok(())
    }

    async fn emit_progress(&self, task_id: &str) -> Result<()> {
        if let Some(task) = self.state.tasks.lock().unwrap().get(task_id).cloned() {
            self.app_handle.emit("download-progress", task)?;
        }
        Ok(())
    }
}

async fn run_download_pipeline(
    app_handle: &tauri::AppHandle,
    state: &State<'_, DownloadState>,
    task_id: &str,
    request: StartDownloadRequest,
    control: TaskControl,
) -> Result<()> {
    update_task_and_emit(state, app_handle, task_id, |task| {
        task.status = TaskStatus::Downloading;
    }, false);

    // 验证 URL 可访问性
    eprintln!("验证视频 URL: {}", request.video_url.chars().take(80).collect::<String>());
    eprintln!("验证音频 URL: {}", request.audio_url.chars().take(80).collect::<String>());

    let temp_dir = task_temp_dir(task_id);
    tokio::fs::create_dir_all(&temp_dir).await?;

    let video_part = temp_dir.join("video.part");
    let audio_part = temp_dir.join("audio.part");
    let downloader = ChunkedDownloader::new(request.config.clone());

    let video_future = async {
        downloader
            .download_stream_to_part(&request.video_url, &video_part, &control, |downloaded, total| {
                update_task_and_emit(state, app_handle, task_id, |task| {
                    task.video_downloaded = downloaded;
                    task.video_size = total;
                    task.video_progress = if total == 0 {
                        0.0
                    } else {
                        (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    if task.status != TaskStatus::Paused {
                        task.status = TaskStatus::Downloading;
                    }
                }, false);
            })
            .await
            .with_context(|| format!("视频下载失败\nURL: {}", request.video_url))
    };

    let audio_future = async {
        downloader
            .download_stream_to_part(&request.audio_url, &audio_part, &control, |downloaded, total| {
                update_task_and_emit(state, app_handle, task_id, |task| {
                    task.audio_downloaded = downloaded;
                    task.audio_size = total;
                    task.audio_progress = if total == 0 {
                        0.0
                    } else {
                        (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    if task.status != TaskStatus::Paused {
                        task.status = TaskStatus::Downloading;
                    }
                }, false);
            })
            .await
            .with_context(|| format!("音频下载失败\nURL: {}", request.audio_url))
    };

    let (video_result, audio_result) = tokio::try_join!(video_future, audio_future)?;

    update_task_and_emit(state, app_handle, task_id, |task| {
        task.status = TaskStatus::Merging;
    }, false);

    // 从任务状态中获取正确的保存路径（已包含合集子目录）
    let save_path = {
        let tasks = state.tasks.lock().unwrap();
        tasks.get(task_id).map(|t| t.save_path.clone())
            .unwrap_or_else(|| request.config.save_path.clone())
    };

    let ffmpeg_path = FFmpegDetector::new(
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("bilibili-downloader"),
    )
    .get_or_install_ffmpeg()
    .await
    .context("未找到可用 FFmpeg，请检查网络或先手动安装 ffmpeg")?;

    let output_dir = PathBuf::from(&save_path);
    tokio::fs::create_dir_all(&output_dir).await?;

    let final_filename = build_filename(&request.title, request.part_title.as_deref());
    let output_path = output_dir.join(final_filename);
    let merger = VideoMerger::new(ffmpeg_path);
    let (merge_tx, _merge_rx) = tokio::sync::mpsc::channel(1);
    merger
        .merge(
            &video_result.output_paths,
            &audio_result.output_paths,
            &output_path,
            merge_tx,
        )
        .await
        .context("音视频合并失败")?;

    update_task_and_emit(state, app_handle, task_id, |task| {
        task.video_progress = 1.0;
        task.audio_progress = 1.0;
        task.video_downloaded = video_result.downloaded;
        task.audio_downloaded = audio_result.downloaded;
        task.video_size = video_result.total;
        task.audio_size = audio_result.total;
        task.status = TaskStatus::Completed;
    }, true); // 保存完成状态

    tokio::fs::remove_dir_all(&temp_dir).await.ok();
    Ok(())
}

fn update_task_and_emit<F>(
    state: &State<'_, DownloadState>,
    app_handle: &tauri::AppHandle,
    task_id: &str,
    update: F,
    save: bool,
) where
    F: FnOnce(&mut DownloadTask),
{
    let (task_snapshot, should_add_history) = {
        let mut tasks = state.tasks.lock().unwrap();
        if let Some(task) = tasks.get_mut(task_id) {
            let old_status = task.status.clone();
            update(task);
            task.updated_at = current_timestamp();

            // 计算速度
            let current_time = task.updated_at;
            let current_downloaded = task.video_downloaded + task.audio_downloaded;

            if let Some(last_time) = task.last_speed_update_time {
                let time_diff = current_time.saturating_sub(last_time);
                let downloaded_diff = current_downloaded.saturating_sub(task.last_speed_downloaded);

                // 只在有数据下载且时间差大于0.5秒时才更新速度，避免跳动
                if downloaded_diff > 0 && time_diff > 500 {
                    // 计算速度（字节/秒）
                    let speed = (downloaded_diff as f64 / time_diff as f64) as u64;
                    task.speed = speed;
                    task.last_speed_update_time = Some(current_time);
                    task.last_speed_downloaded = current_downloaded;
                }
            } else {
                // 首次更新速度计算
                task.last_speed_update_time = Some(current_time);
                task.last_speed_downloaded = current_downloaded;
            }

            // 检查是否应该添加历史记录（状态变为Completed或Failed）
            let was_not_finished = !matches!(old_status, TaskStatus::Completed | TaskStatus::Failed(_));
            let is_now_finished = matches!(task.status, TaskStatus::Completed | TaskStatus::Failed(_));
            let should_add = was_not_finished && is_now_finished;

            (Some(task.clone()), should_add)
        } else {
            (None, false)
        }
    };

    if let Some(ref task) = task_snapshot {
        let _ = app_handle.emit("download-progress", task.clone());

        // 根据参数决定是否保存到文件
        if save {
            let tasks = state.tasks.lock().unwrap();
            let _ = persistence::save_tasks(&tasks);
        }

        // 添加历史记录
        if should_add_history {
            let entry = create_history_entry(task);
            let _ = history::add_history_entry(entry);
            eprintln!("✓ 已添加历史记录: {}", task.title);
        }
    }
}

fn task_temp_dir(task_id: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("bilibili-downloader")
        .join("tasks")
        .join(task_id)
}

fn build_filename(title: &str, part_title: Option<&str>) -> String {
    let base = part_title.unwrap_or(title);
    let sanitized = sanitize_filename(base);
    format!("{}.mp4", sanitized)
}

fn sanitize_filename(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if c.is_control() => '_',
            _ => c,
        })
        .collect();

    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "未命名视频".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 计算保存路径：如果是合集，创建合集子目录
fn compute_save_path(base_path: &str, collection_type: &Option<String>, collection_title: &Option<String>) -> String {
    // 只有多分P和合集才创建子目录
    let is_collection = matches!(
        collection_type.as_deref(),
        Some("multi_part") | Some("collection")
    );

    if is_collection {
        if let Some(title) = collection_title {
            if !title.trim().is_empty() {
                let sanitized_dir = sanitize_filename_for_dir(title);
                let path = std::path::PathBuf::from(base_path).join(&sanitized_dir);

                // 确保目录存在
                if let Err(e) = std::fs::create_dir_all(&path) {
                    eprintln!("创建合集目录失败: {}, 使用基础路径: {}", e, path.display());
                    return base_path.to_string();
                }

                return path.to_string_lossy().to_string();
            }
        }
    }

    base_path.to_string()
}

/// 清理目录名称（移除不适合作为目录名的字符）
fn sanitize_filename_for_dir(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if c.is_control() => '_',
            _ => c,
        })
        .collect();

    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "未命名合集".to_string()
    } else {
        // 移除首尾的点和空格
        trimmed.trim_matches('.').trim().to_string()
    }
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn create_history_entry(task: &DownloadTask) -> HistoryEntry {
    let (status, completed_at, error_message) = match &task.status {
        TaskStatus::Completed => (
            "Completed".to_string(),
            Some(task.updated_at),
            None,
        ),
        TaskStatus::Failed(msg) => (
            format!("Failed:{}", msg),
            Some(task.updated_at),
            Some(msg.clone()),
        ),
        _ => (
            "Pending".to_string(),
            None,
            None,
        ),
    };

    HistoryEntry {
        task_id: task.task_id.clone(),
        bvid: task.bvid.clone(),
        cid: task.cid,
        title: task.title.clone(),
        part_title: task.part_title.clone(),
        status,
        video_size: task.video_size,
        audio_size: task.audio_size,
        total_size: task.video_size + task.audio_size,
        save_path: task.save_path.clone(),
        filename: task.filename.clone(),
        created_at: task.created_at,
        completed_at,
        error_message,
    }
}
