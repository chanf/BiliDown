pub mod manager;
pub mod task;
pub mod chunked;
pub mod resume;
pub mod merger;

pub use manager::DownloadManager;
pub use task::DownloadTaskRunner;
pub use chunked::{ChunkedDownloader, ProgressUpdate, ChunkInfo};
pub use resume::{ResumeManager, ResumeMetadata};
pub use merger::VideoMerger;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::task::JoinHandle;

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Downloading,
    Paused,
    Merging,
    Completed,
    Failed(String),
}

/// 下载任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub task_id: String,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub status: TaskStatus,
    pub video_progress: f32,
    pub audio_progress: f32,
    pub video_size: u64,
    pub audio_size: u64,
    pub video_downloaded: u64,
    pub audio_downloaded: u64,
    pub speed: u64,
    pub save_path: String,
    pub filename: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 下载配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub save_path: String,
    pub concurrent_connections: usize,
    pub chunk_size: usize,
    pub quality: i32,
    pub max_retry: usize,
    pub timeout: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            save_path: dirs::home_dir()
                .unwrap()
                .join("Movies")
                .to_string_lossy()
                .to_string(),
            concurrent_connections: 4,
            chunk_size: 1024 * 1024,
            quality: 80,
            max_retry: 3,
            timeout: 30,
        }
    }
}

/// 全局下载状态
pub struct DownloadState {
    pub tasks: Mutex<HashMap<String, DownloadTask>>,
    pub active_tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    pub config: Mutex<DownloadConfig>,
    pub ffmpeg_path: Mutex<Option<String>>,
}

