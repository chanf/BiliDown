use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

use crate::downloader::{ChunkedDownloader, ProgressUpdate, ResumeMetadata, VideoMerger, DownloadConfig, TaskStatus};
use crate::downloader::resume::ResumeManager;

pub struct DownloadTaskRunner {
    task_id: String,
    metadata: ResumeMetadata,
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    config: DownloadConfig,
    resume_manager: ResumeManager,
    ffmpeg_path: String,
    app_handle: tauri::AppHandle,
}

impl DownloadTaskRunner {
    pub fn new(
        task_id: String,
        metadata: ResumeMetadata,
        resume_manager: ResumeManager,
        ffmpeg_path: String,
        app_handle: tauri::AppHandle,
    ) -> Self {
        let config = metadata.config.clone();
        Self {
            task_id,
            metadata,
            cancelled: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            config,
            resume_manager,
            ffmpeg_path,
            app_handle,
        }
    }

    pub async fn run(&self) -> Result<()> {
        self.emit_status(TaskStatus::Downloading).await;

        let output_dir = std::path::Path::new(&self.config.save_path);
        tokio::fs::create_dir_all(output_dir).await?;

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<crate::downloader::ProgressUpdate>(100);

        let cancel_token = self.cancelled.clone();
        let paused_token = self.paused.clone();

        tokio::spawn(async move {
            let mut video_downloaded: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
            let mut audio_downloaded: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
            let mut total_video = 0u64;
            let mut total_audio = 0u64;

            while !cancel_token.load(Ordering::SeqCst) {
                tokio::select! {
                    Some(update) = progress_rx.recv() => {
                        if update.chunk_index < 10000 {
                            video_downloaded.insert(update.chunk_index, update.downloaded);
                            total_video = update.total;
                        } else {
                            audio_downloaded.insert(update.chunk_index - 10000, update.downloaded);
                            total_audio = update.total;
                        }
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                        while paused_token.load(Ordering::SeqCst) {
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        });

        let downloader = ChunkedDownloader::new(self.config.clone());
        let temp_dir = self.resume_manager.get_temp_dir(&self.task_id);

        if self.should_cancel() {
            self.emit_status(TaskStatus::Failed("任务已取消".to_string())).await;
            return Ok(());
        }

        let video_paths = downloader.download_chunked(
            &self.metadata.video_url,
            self.metadata.video_size,
            &temp_dir,
            &format!("{}_video", self.task_id),
            progress_tx.clone(),
            &self.cancelled,
            &self.paused,
        ).await;

        if let Err(e) = video_paths {
            self.emit_status(TaskStatus::Failed(format!("视频下载失败: {}", e))).await;
            anyhow::bail!("视频下载失败: {}", e);
        }

        let video_paths = video_paths.unwrap();

        if self.should_cancel() {
            self.emit_status(TaskStatus::Failed("任务已取消".to_string())).await;
            return Ok(());
        }

        let audio_paths = downloader.download_chunked(
            &self.metadata.audio_url,
            self.metadata.audio_size,
            &temp_dir,
            &format!("{}_audio", self.task_id),
            progress_tx.clone(),
            &self.cancelled,
            &self.paused,
        ).await;

        if let Err(e) = audio_paths {
            self.emit_status(TaskStatus::Failed(format!("音频下载失败: {}", e))).await;
            anyhow::bail!("音频下载失败: {}", e);
        }

        let audio_paths = audio_paths.unwrap();

        if self.should_cancel() {
            self.emit_status(TaskStatus::Failed("任务已取消".to_string())).await;
            return Ok(());
        }

        self.emit_status(TaskStatus::Merging).await;

        let merger = VideoMerger::new(self.ffmpeg_path.clone());
        let output_filename = format!("{}.mp4",
            self.metadata.title.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "_")
        );
        let output_path = output_dir.join(&output_filename);

        let (merge_progress_tx, _) = tokio::sync::mpsc::channel(10);

        if let Err(e) = merger.merge(
            &video_paths,
            &audio_paths,
            &output_path,
            merge_progress_tx,
        ).await {
            self.emit_status(TaskStatus::Failed(format!("合并失败: {}", e))).await;
            anyhow::bail!("合并失败: {}", e);
        }

        if let Err(e) = self.resume_manager.cleanup_temp_files(&self.task_id).await {
            eprintln!("清理临时文件失败: {}", e);
        }

        self.emit_status(TaskStatus::Completed).await;

        Ok(())
    }

    async fn emit_status(&self, status: TaskStatus) {
        let task = crate::downloader::DownloadTask {
            task_id: self.task_id.clone(),
            bvid: self.metadata.bvid.clone(),
            cid: self.metadata.cid,
            title: self.metadata.title.clone(),
            part_title: self.metadata.part_title.clone(),
            status,
            video_progress: 0.0,
            audio_progress: 0.0,
            video_size: self.metadata.video_size,
            audio_size: self.metadata.audio_size,
            video_downloaded: 0,
            audio_downloaded: 0,
            speed: 0,
            save_path: self.config.save_path.clone(),
            filename: format!("{}.mp4", self.metadata.title),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };
        let _ = self.app_handle.emit("download-progress", task);
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn should_pause(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn should_cancel(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
