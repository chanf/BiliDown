use anyhow::Result;
use tauri::{Emitter, State};
use std::sync::Arc;

use crate::downloader::{DownloadConfig, DownloadState, DownloadTask, TaskStatus};

/// 下载任务管理器
pub struct DownloadManager<'a> {
    state: &'a State<'a, DownloadState>,
    app_handle: tauri::AppHandle,
}

impl<'a> DownloadManager<'a> {
    pub fn new(state: &'a State<'a, DownloadState>, app_handle: tauri::AppHandle) -> Self {
        Self {
            state,
            app_handle,
        }
    }

    /// 添加下载任务
    pub async fn add_task(
        &self,
        bvid: &str,
        cid: i64,
        title: &str,
        part_title: Option<String>,
    ) -> Result<String> {
        let task_id = format!("{}_{}", bvid, cid);
        let config = self.state.config.lock().unwrap().clone();

        let task = DownloadTask {
            task_id: task_id.clone(),
            bvid: bvid.to_string(),
            cid,
            title: title.to_string(),
            part_title,
            status: TaskStatus::Pending,
            video_progress: 0.0,
            audio_progress: 0.0,
            video_size: 0,
            audio_size: 0,
            video_downloaded: 0,
            audio_downloaded: 0,
            speed: 0,
            save_path: config.save_path.clone(),
            filename: format!("{}.mp4", title),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };

        self.state.tasks.lock().unwrap().insert(task_id.clone(), task);

        // 发送任务创建事件
        self.emit_progress(&task_id).await?;

        Ok(task_id)
    }

    /// 暂停任务
    pub async fn pause_task(&self, task_id: &str) -> Result<()> {
        {
            if let Some(task) = self.state.tasks.lock().unwrap().get_mut(task_id) {
                task.status = TaskStatus::Paused;
            }
        }
        self.emit_progress(task_id).await?;
        Ok(())
    }

    /// 恢复任务
    pub async fn resume_task(&self, task_id: &str) -> Result<()> {
        {
            if let Some(task) = self.state.tasks.lock().unwrap().get_mut(task_id) {
                task.status = TaskStatus::Downloading;
            }
        }
        self.emit_progress(task_id).await?;
        Ok(())
    }

    /// 删除任务
    pub async fn delete_task(&self, task_id: &str, _clean_files: bool) -> Result<()> {
        // 移除任务
        self.state.tasks.lock().unwrap().remove(task_id);

        // 停止正在运行的任务
        if let Some(handle) = self.state.active_tasks.lock().unwrap().remove(task_id) {
            handle.abort();
        }

        // 清理文件
        // TODO: 实现文件清理

        Ok(())
    }

    /// 获取所有任务
    pub async fn get_tasks(&self) -> Vec<DownloadTask> {
        self.state
            .tasks
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// 更新任务进度
    pub async fn update_task_progress(&self, task_id: &str) -> Result<()> {
        self.emit_progress(task_id).await?;
        Ok(())
    }

    /// 发送进度事件到前端
    async fn emit_progress(&self, task_id: &str) -> Result<()> {
        if let Some(task) = self.state.tasks.lock().unwrap().get(task_id) {
            self.app_handle
                .emit("download-progress", task)
                .map_err(|e| anyhow::anyhow!("Failed to emit progress: {}", e))?;
        }
        Ok(())
    }
}
