use reqwest::Client;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Semaphore;

use crate::downloader::DownloadConfig;

#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub chunk_index: usize,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChunkInfo {
    pub index: usize,
    pub start: u64,
    pub end: u64,
    pub downloaded: bool,
    pub path: String,
}

pub struct ChunkedDownloader {
    client: Client,
    config: DownloadConfig,
}

impl ChunkedDownloader {
    pub fn new(config: DownloadConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    pub async fn download_chunked(
        &self,
        url: &str,
        total_size: u64,
        output_dir: &std::path::Path,
        task_id: &str,
        progress_tx: tokio::sync::mpsc::Sender<ProgressUpdate>,
        cancel_token: &Arc<AtomicBool>,
        pause_token: &Arc<AtomicBool>,
    ) -> Result<Vec<PathBuf>> {
        let chunk_size = self.config.chunk_size as u64;
        let num_chunks = (total_size + chunk_size - 1) / chunk_size;

        let mut chunks = Vec::new();
        for i in 0..num_chunks {
            let start = i * chunk_size;
            let end = std::cmp::min(start + chunk_size, total_size);

            chunks.push(ChunkInfo {
                index: i as usize,
                start,
                end,
                downloaded: false,
                path: format!("{}/{}_chunk_{}.tmp", output_dir.display(), task_id, i),
            });
        }

        tokio::fs::create_dir_all(output_dir).await?;

        let semaphore = Arc::new(Semaphore::new(self.config.concurrent_connections));
        let mut download_tasks = Vec::new();

        for chunk in chunks {
            let cancel_token = cancel_token.clone();
            let pause_token = pause_token.clone();
            let url = url.to_string();
            let client = self.client.clone();
            let tx = progress_tx.clone();
            let semaphore = semaphore.clone();
            let max_retry = self.config.max_retry;
            let timeout = self.config.timeout;

            let task = tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();

                while pause_token.load(Ordering::SeqCst) {
                    if cancel_token.load(Ordering::SeqCst) {
                        return Err(anyhow::anyhow!("任务已取消"));
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }

                if cancel_token.load(Ordering::SeqCst) {
                    return Err(anyhow::anyhow!("任务已取消"));
                }

                Self::download_chunk_with_retry(
                    &client,
                    &url,
                    &chunk,
                    &cancel_token,
                    &pause_token,
                    tx,
                    max_retry,
                    timeout,
                ).await
            });

            download_tasks.push(task);
        }

        let results = futures::future::join_all(download_tasks).await;
        let mut paths = Vec::new();

        for result in results {
            match result {
                Ok(path) => paths.push(path?),
                Err(e) => anyhow::bail!("分块下载失败: {}", e),
            }
        }

        Ok(paths)
    }

    async fn download_chunk_with_retry(
        client: &Client,
        url: &str,
        chunk: &ChunkInfo,
        cancel_token: &Arc<AtomicBool>,
        pause_token: &Arc<AtomicBool>,
        progress_tx: tokio::sync::mpsc::Sender<ProgressUpdate>,
        max_retry: usize,
        timeout: u64,
    ) -> Result<PathBuf> {
        let mut last_error = None;

        for attempt in 0..=max_retry {
            if cancel_token.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("任务已取消"));
            }

            while pause_token.load(Ordering::SeqCst) {
                if cancel_token.load(Ordering::SeqCst) {
                    return Err(anyhow::anyhow!("任务已取消"));
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            match Self::download_chunk(client, url, chunk, progress_tx.clone(), timeout).await {
                Ok(path) => return Ok(path),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retry {
                    let delay = 2u64.pow(attempt as u32);
                        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("下载失败")))
    }

    async fn download_chunk(
        client: &Client,
        url: &str,
        chunk: &ChunkInfo,
        progress_tx: tokio::sync::mpsc::Sender<ProgressUpdate>,
        timeout: u64,
    ) -> Result<PathBuf> {
        let range = format!("bytes={}-{}", chunk.start, chunk.end - 1);

        let response = client
            .get(url)
            .header("Range", &range)
            .header("Referer", "https://www.bilibili.com")
            .timeout(std::time::Duration::from_secs(timeout))
            .send()
            .await?;

        if !response.status().is_success() && response.status().as_u16() != 206 {
            anyhow::bail!("下载失败: {}", response.status());
        }

        let bytes = response.bytes().await?;

        std::fs::write(&chunk.path, &bytes)?;

        progress_tx
            .send(ProgressUpdate {
                chunk_index: chunk.index,
                downloaded: bytes.len() as u64,
                total: chunk.end - chunk.start,
            })
            .await
            .ok();

        Ok(PathBuf::from(&chunk.path))
    }
}
