use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::downloader::DownloadTask;

/// 持久化数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedData {
    /// 版本号，用于未来格式升级
    pub version: u32,
    /// 下载任务列表
    pub tasks: HashMap<String, PersistedTask>,
    /// 最后更新时间
    pub last_updated: i64,
}

/// 持久化的任务（简化版，只保留必要信息）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTask {
    pub task_id: String,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub status: String,
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

impl From<&DownloadTask> for PersistedTask {
    fn from(task: &DownloadTask) -> Self {
        let status = match &task.status {
            crate::downloader::TaskStatus::Pending => "Pending".to_string(),
            crate::downloader::TaskStatus::Downloading => "Downloading".to_string(),
            crate::downloader::TaskStatus::Paused => "Paused".to_string(),
            crate::downloader::TaskStatus::Merging => "Merging".to_string(),
            crate::downloader::TaskStatus::Completed => "Completed".to_string(),
            crate::downloader::TaskStatus::Failed(msg) => format!("Failed:{}", msg),
        };

        Self {
            task_id: task.task_id.clone(),
            bvid: task.bvid.clone(),
            cid: task.cid,
            title: task.title.clone(),
            part_title: task.part_title.clone(),
            status,
            video_progress: task.video_progress,
            audio_progress: task.audio_progress,
            video_size: task.video_size,
            audio_size: task.audio_size,
            video_downloaded: task.video_downloaded,
            audio_downloaded: task.audio_downloaded,
            speed: task.speed,
            save_path: task.save_path.clone(),
            filename: task.filename.clone(),
            created_at: task.created_at,
            updated_at: task.updated_at,
        }
    }
}

impl PersistedTask {
    /// 转换为 DownloadTask，恢复状态为 Pending（需要重新下载）
    pub fn to_pending_task(&self) -> DownloadTask {
        let status = if self.status.starts_with("Failed:") {
            crate::downloader::TaskStatus::Failed(
                self.status.strip_prefix("Failed:").unwrap_or("下载失败").to_string()
            )
        } else if self.status == "Completed" {
            crate::downloader::TaskStatus::Completed
        } else if self.status == "Paused" {
            crate::downloader::TaskStatus::Paused
        } else {
            // 对于下载中、合并中等状态，恢复为 Pending
            crate::downloader::TaskStatus::Pending
        };

        DownloadTask {
            task_id: self.task_id.clone(),
            bvid: self.bvid.clone(),
            cid: self.cid,
            title: self.title.clone(),
            part_title: self.part_title.clone(),
            status,
            video_progress: self.video_progress,
            audio_progress: self.audio_progress,
            video_size: self.video_size,
            audio_size: self.audio_size,
            video_downloaded: self.video_downloaded,
            audio_downloaded: self.audio_downloaded,
            speed: 0, // 恢复时速度重置为0
            save_path: self.save_path.clone(),
            filename: self.filename.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_speed_update_time: None,
            last_speed_downloaded: 0,
        }
    }
}

/// 获取持久化文件路径
pub fn get_persistence_file() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .context("无法获取数据目录")?;

    let app_dir = data_dir.join("bilibili-downloader");
    fs::create_dir_all(&app_dir)
        .context("创建应用数据目录失败")?;

    Ok(app_dir.join("tasks.json"))
}

/// 保存任务到文件
pub fn save_tasks(tasks: &HashMap<String, DownloadTask>) -> Result<()> {
    let file_path = get_persistence_file()?;

    let persisted_tasks: HashMap<String, PersistedTask> = tasks
        .iter()
        .filter(|(_, task)| {
            // 只保存非完成状态的任务，或者最近24小时内完成的任务
            let is_recent = task.updated_at > current_timestamp() - 86400;
            !matches!(task.status, crate::downloader::TaskStatus::Completed) || is_recent
        })
        .map(|(id, task)| (id.clone(), PersistedTask::from(task)))
        .collect();

    let data = PersistedData {
        version: 1,
        tasks: persisted_tasks,
        last_updated: current_timestamp(),
    };

    let json = serde_json::to_string_pretty(&data)
        .context("序列化任务数据失败")?;

    // 原子写入：先写临时文件，再重命名
    let temp_path = file_path.with_extension("json.tmp");
    fs::write(&temp_path, json)
        .context("写入任务文件失败")?;

    fs::rename(&temp_path, &file_path)
        .context("保存任务文件失败")?;

    eprintln!("✓ 已保存 {} 个任务到文件", data.tasks.len());
    Ok(())
}

/// 从文件加载任务
pub fn load_tasks() -> Result<HashMap<String, PersistedTask>> {
    let file_path = get_persistence_file()?;

    if !file_path.exists() {
        eprintln!("未找到任务持久化文件，使用空任务列表");
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&file_path)
        .context("读取任务文件失败")?;

    let data: PersistedData = serde_json::from_str(&content)
        .context("解析任务文件失败")?;

    eprintln!("✓ 已从文件加载 {} 个任务（版本 {}）", data.tasks.len(), data.version);
    Ok(data.tasks)
}

/// 清理过期的已完成任务
pub fn cleanup_old_tasks() -> Result<()> {
    let file_path = get_persistence_file()?;

    if !file_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&file_path)
        .context("读取任务文件失败")?;

    let mut data: PersistedData = serde_json::from_str(&content)
        .context("解析任务文件失败")?;

    let before_count = data.tasks.len();
    let cutoff_time = current_timestamp() - 86400 * 7; // 7天前

    // 移除7天前已完成的任务
    data.tasks.retain(|_, task| {
        !(task.status == "Completed" && task.updated_at < cutoff_time)
    });

    let after_count = data.tasks.len();

    if before_count != after_count {
        data.last_updated = current_timestamp();
        let json = serde_json::to_string_pretty(&data)
            .context("序列化任务数据失败")?;

        let temp_path = file_path.with_extension("json.tmp");
        fs::write(&temp_path, json)
            .context("写入任务文件失败")?;

        fs::rename(&temp_path, &file_path)
            .context("保存任务文件失败")?;

        eprintln!("✓ 已清理 {} 个过期任务", before_count - after_count);
    }

    Ok(())
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
