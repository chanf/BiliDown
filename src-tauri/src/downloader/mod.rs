pub mod manager;
pub mod chunked;
pub mod merger;

pub use manager::DownloadManager;
pub use manager::StartDownloadRequest;
pub use chunked::ChunkedDownloader;
pub use merger::VideoMerger;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    Arc,
    Mutex,
    atomic::AtomicBool,
};
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
    // 速度计算辅助字段（不序列化）
    #[serde(skip)]
    pub last_speed_update_time: Option<i64>,
    #[serde(skip)]
    pub last_speed_downloaded: u64,
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

pub const CONFIG_FILE: &str = "config.json";

impl DownloadConfig {
    /// 获取配置文件路径
    pub fn get_config_file() -> std::path::PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap())
            .join("bilibili-downloader");

        // 确保配置目录存在
        std::fs::create_dir_all(&config_dir).ok();

        config_dir.join(CONFIG_FILE)
    }

    /// 从文件加载配置
    pub fn load_from_file() -> Self {
        let config_file = Self::get_config_file();

        if config_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&config_file) {
                if let Ok(config) = serde_json::from_str::<DownloadConfig>(&content) {
                    return config;
                }
            }
        }

        Self::default()
    }

    /// 保存配置到文件
    pub fn save_to_file(&self) -> anyhow::Result<()> {
        let config_file = Self::get_config_file();
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_file, content)?;
        Ok(())
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            save_path: dirs::home_dir()
                .unwrap()
                .join("Movies")
                .join("DiliDown")
                .to_string_lossy()
                .to_string(),
            concurrent_connections: 8,   // 增加到 8 并发，提升下载速度
            chunk_size: 5 * 1024 * 1024, // 增加到 5MB，减少分块数量，提升效率
            quality: 120, // 默认 4K，会自动降级到最高可用质量
            max_retry: 3,  // 减少到 3 次，避免过长等待
            timeout: 30,  // 减少到 30 秒，快速失败
        }
    }
}

/// 全局下载状态
pub struct DownloadState {
    pub tasks: Mutex<HashMap<String, DownloadTask>>,
    pub active_tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    pub controls: Mutex<HashMap<String, TaskControl>>,
    pub config: Mutex<DownloadConfig>,
}

#[derive(Debug, Clone)]
pub struct TaskControl {
    pub paused: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
}

impl TaskControl {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}
