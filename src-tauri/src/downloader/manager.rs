use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};

use crate::downloader::{
    ChunkedDownloader, DownloadConfig, DownloadState, DownloadTask, TaskControl, TaskStatus,
    VideoMerger,
};
use crate::ffmpeg::FFmpegDetector;

#[derive(Debug, Clone)]
pub struct StartDownloadRequest {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub video_url: String,
    pub audio_url: String,
    pub config: DownloadConfig,
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
            save_path: request.config.save_path.clone(),
            filename,
            created_at: now,
            updated_at: now,
        };

        self.state.tasks.lock().unwrap().insert(task_id.clone(), task);
        let control = TaskControl::new();
        self.state
            .controls
            .lock()
            .unwrap()
            .insert(task_id.clone(), control.clone());
        self.emit_progress(&task_id).await?;

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
                    });
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
        });
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
        });
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
    });

    let temp_dir = task_temp_dir(task_id);
    tokio::fs::create_dir_all(&temp_dir).await?;

    let video_part = temp_dir.join("video.part");
    let audio_part = temp_dir.join("audio.part");
    let downloader = ChunkedDownloader::new(request.config.clone());

    let video_result = downloader
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
            });
        })
        .await
        .context("视频下载失败")?;

    let audio_result = downloader
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
            });
        })
        .await
        .context("音频下载失败")?;

    update_task_and_emit(state, app_handle, task_id, |task| {
        task.status = TaskStatus::Merging;
    });

    let ffmpeg_path = FFmpegDetector::new(
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("bilibili-downloader"),
    )
    .get_or_install_ffmpeg()
    .await
    .unwrap_or_else(|_| "ffmpeg".to_string());

    let output_dir = PathBuf::from(&request.config.save_path);
    tokio::fs::create_dir_all(&output_dir).await?;

    let final_filename = build_filename(&request.title, request.part_title.as_deref());
    let output_path = output_dir.join(final_filename);
    let merger = VideoMerger::new(ffmpeg_path);
    let (merge_tx, _merge_rx) = tokio::sync::mpsc::channel(1);
    merger
        .merge(
            &[video_result.output_path.clone()],
            &[audio_result.output_path.clone()],
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
    });

    tokio::fs::remove_dir_all(&temp_dir).await.ok();
    Ok(())
}

fn update_task_and_emit<F>(
    state: &State<'_, DownloadState>,
    app_handle: &tauri::AppHandle,
    task_id: &str,
    update: F,
) where
    F: FnOnce(&mut DownloadTask),
{
    let snapshot = {
        let mut tasks = state.tasks.lock().unwrap();
        if let Some(task) = tasks.get_mut(task_id) {
            update(task);
            task.updated_at = current_timestamp();
            Some(task.clone())
        } else {
            None
        }
    };

    if let Some(task) = snapshot {
        let _ = app_handle.emit("download-progress", task);
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

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
