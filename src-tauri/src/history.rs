use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 历史记录条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub task_id: String,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub status: String,
    pub video_size: u64,
    pub audio_size: u64,
    pub total_size: u64,
    pub save_path: String,
    pub filename: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error_message: Option<String>,
}

/// 历史记录数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryData {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
    pub last_updated: i64,
}

/// 统计数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadStatistics {
    pub total_downloads: usize,
    pub completed_downloads: usize,
    pub failed_downloads: usize,
    pub total_bytes: u64,
    pub success_rate: f32,
    pub average_speed: u64,
    pub last_7_days: usize,
    pub last_30_days: usize,
}

/// 获取历史记录文件路径
pub fn get_history_file() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .context("无法获取数据目录")?;

    let app_dir = data_dir.join("bilibili-downloader");
    fs::create_dir_all(&app_dir)
        .context("创建应用数据目录失败")?;

    Ok(app_dir.join("history.json"))
}

/// 添加历史记录
pub fn add_history_entry(entry: HistoryEntry) -> Result<()> {
    let mut data = load_history_data()?;

    // 检查是否已存在相同 task_id 的记录
    if let Some(pos) = data.entries.iter().position(|e| e.task_id == entry.task_id) {
        // 更新现有记录
        data.entries[pos] = entry;
    } else {
        // 添加新记录
        data.entries.push(entry);
    }

    // 按创建时间倒序排序
    data.entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // 限制历史记录数量（最多保留1000条）
    if data.entries.len() > 1000 {
        data.entries.truncate(1000);
    }

    data.last_updated = current_timestamp();
    save_history_data(&data)?;

    Ok(())
}

/// 加载历史记录数据
pub fn load_history_data() -> Result<HistoryData> {
    let file_path = get_history_file()?;

    if !file_path.exists() {
        return Ok(HistoryData {
            version: 1,
            entries: Vec::new(),
            last_updated: current_timestamp(),
        });
    }

    let content = fs::read_to_string(&file_path)
        .context("读取历史记录文件失败")?;

    let data: HistoryData = serde_json::from_str(&content)
        .context("解析历史记录文件失败")?;

    Ok(data)
}

/// 保存历史记录数据
fn save_history_data(data: &HistoryData) -> Result<()> {
    let file_path = get_history_file()?;

    let json = serde_json::to_string_pretty(data)
        .context("序列化历史记录失败")?;

    // 原子写入
    let temp_path = file_path.with_extension("json.tmp");
    fs::write(&temp_path, json)
        .context("写入历史记录文件失败")?;

    fs::rename(&temp_path, &file_path)
        .context("保存历史记录文件失败")?;

    Ok(())
}

/// 搜索历史记录
pub fn search_history(
    keyword: Option<String>,
    status: Option<String>,
    start_date: Option<i64>,
    end_date: Option<i64>,
    limit: usize,
) -> Result<Vec<HistoryEntry>> {
    let data = load_history_data()?;

    let results: Vec<HistoryEntry> = data.entries
        .into_iter()
        .filter(|entry| {
            // 关键词过滤
            if let Some(ref kw) = keyword {
                let kw_lower = kw.to_lowercase();
                let title_match = entry.title.to_lowercase().contains(&kw_lower);
                let part_match = entry.part_title.as_ref()
                    .map(|t| t.to_lowercase().contains(&kw_lower))
                    .unwrap_or(false);
                if !title_match && !part_match {
                    return false;
                }
            }

            // 状态过滤
            if let Some(ref st) = status {
                if entry.status != *st {
                    return false;
                }
            }

            // 日期范围过滤
            if let Some(start) = start_date {
                if entry.created_at < start {
                    return false;
                }
            }
            if let Some(end) = end_date {
                if entry.created_at > end {
                    return false;
                }
            }

            true
        })
        .take(limit)
        .collect();

    Ok(results)
}

/// 计算统计数据
pub fn calculate_statistics() -> Result<DownloadStatistics> {
    let data = load_history_data()?;

    let total_downloads = data.entries.len();
    let completed_downloads = data.entries.iter()
        .filter(|e| e.status == "Completed")
        .count();
    let failed_downloads = data.entries.iter()
        .filter(|e| e.status.starts_with("Failed"))
        .count();

    let total_bytes: u64 = data.entries.iter()
        .filter(|e| e.status == "Completed")
        .map(|e| e.total_size)
        .sum();

    let success_rate = if total_downloads > 0 {
        (completed_downloads as f32 / total_downloads as f32) * 100.0
    } else {
        0.0
    };

    // 计算平均速度（仅计算已完成的任务）
    let average_speed = if completed_downloads > 0 {
        let total_speed: u64 = data.entries.iter()
            .filter(|e| e.status == "Completed")
            .map(|e| {
                // 简单估算：总大小 / 完成时间（假设平均下载时间为5分钟）
                e.total_size / 300
            })
            .sum();
        total_speed / completed_downloads as u64
    } else {
        0
    };

    // 计算最近7天和30天的下载数量
    let now = current_timestamp();
    let seven_days_ago = now - 7 * 86400;
    let thirty_days_ago = now - 30 * 86400;

    let last_7_days = data.entries.iter()
        .filter(|e| e.created_at >= seven_days_ago)
        .count();

    let last_30_days = data.entries.iter()
        .filter(|e| e.created_at >= thirty_days_ago)
        .count();

    Ok(DownloadStatistics {
        total_downloads,
        completed_downloads,
        failed_downloads,
        total_bytes,
        success_rate,
        average_speed,
        last_7_days,
        last_30_days,
    })
}

/// 清理旧历史记录
pub fn cleanup_old_history(days_to_keep: i64) -> Result<usize> {
    let mut data = load_history_data()?;

    let cutoff_time = current_timestamp() - days_to_keep * 86400;
    let before_count = data.entries.len();

    // 只保留最近 N 天的记录
    data.entries.retain(|entry| {
        entry.created_at >= cutoff_time || entry.status != "Completed"
    });

    let after_count = data.entries.len();
    let removed_count = before_count - after_count;

    if removed_count > 0 {
        data.last_updated = current_timestamp();
        save_history_data(&data)?;
    }

    Ok(removed_count)
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
