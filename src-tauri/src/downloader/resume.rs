use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::downloader::{ChunkInfo, DownloadConfig};

/// 断点续传元数据
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ResumeMetadata {
    pub task_id: String,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub video_url: String,
    pub audio_url: String,
    pub video_size: u64,
    pub audio_size: u64,
    pub video_chunks: Vec<ChunkInfo>,
    pub audio_chunks: Vec<ChunkInfo>,
    pub config: DownloadConfig,
    pub created_at: i64,
}

/// 断点续传管理器
pub struct ResumeManager {
    metadata_dir: PathBuf,
    temp_dir: PathBuf,
}

impl ResumeManager {
    pub fn new(base_dir: &Path) -> Self {
        let metadata_dir = base_dir.join("resume");
        let temp_dir = base_dir.join("temp");

        std::fs::create_dir_all(&metadata_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();

        Self {
            metadata_dir,
            temp_dir,
        }
    }

    /// 保存元数据
    pub async fn save_metadata(&self, metadata: &ResumeMetadata) -> Result<()> {
        let path = self.metadata_dir.join(format!("{}.json", metadata.task_id));
        let json = serde_json::to_string_pretty(metadata)?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    /// 加载元数据
    pub async fn load_metadata(&self, task_id: &str) -> Option<ResumeMetadata> {
        let path = self.metadata_dir.join(format!("{}.json", task_id));
        if !path.exists() {
            return None;
        }

        let content = tokio::fs::read_to_string(path).await.ok()?;
        serde_json::from_str(&content).ok()
    }

    /// 删除元数据
    pub async fn delete_metadata(&self, task_id: &str) -> Result<()> {
        let path = self.metadata_dir.join(format!("{}.json", task_id));
        tokio::fs::remove_file(path).await.ok();
        Ok(())
    }

    /// 列出所有可恢复的任务
    pub async fn list_resumable_tasks(&self) -> Vec<ResumeMetadata> {
        let mut tasks = Vec::new();

        if let Ok(mut entries) = tokio::fs::read_dir(&self.metadata_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(ext) = entry.path().extension() {
                    if ext == "json" {
                        if let Ok(content) = tokio::fs::read_to_string(entry.path()).await {
                            if let Ok(metadata) = serde_json::from_str::<ResumeMetadata>(&content) {
                                tasks.push(metadata);
                            }
                        }
                    }
                }
            }
        }

        tasks
    }

    /// 清理临时文件
    pub async fn cleanup_temp_files(&self, task_id: &str) -> Result<()> {
        let task_dir = self.temp_dir.join(task_id);
        if task_dir.exists() {
            tokio::fs::remove_dir_all(&task_dir).await?;
        }
        Ok(())
    }

    /// 获取临时目录
    pub fn get_temp_dir(&self, task_id: &str) -> PathBuf {
        self.temp_dir.join(task_id)
    }

    /// 生成任务ID
    pub fn generate_task_id(&self, bvid: &str, cid: i64) -> String {
        format!("{}_{}", bvid, cid)
    }

    /// 创建新的任务ID
    pub fn create_task_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
