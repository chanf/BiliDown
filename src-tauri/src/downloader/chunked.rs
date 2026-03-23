use anyhow::{Context, Result};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;

use crate::downloader::{DownloadConfig, TaskControl};

const PAUSE_POLL_MS: u64 = 200;

pub struct StreamDownloadResult {
    pub downloaded: u64,
    pub total: u64,
    pub output_path: PathBuf,
}

pub struct ChunkedDownloader {
    client: Client,
    config: DownloadConfig,
}

impl ChunkedDownloader {
    pub fn new(config: DownloadConfig) -> Self {
        Self {
            client: create_download_client(),
            config,
        }
    }

    pub async fn download_stream_to_part<F>(
        &self,
        url: &str,
        output_path: &Path,
        control: &TaskControl,
        mut on_progress: F,
    ) -> Result<StreamDownloadResult>
    where
        F: FnMut(u64, u64),
    {
        if self.config.chunk_size == 0 {
            anyhow::bail!("分块大小不能为 0");
        }

        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let total_size = self.fetch_total_size(url).await?;
        let mut downloaded = self.local_file_size(output_path).await?;

        if downloaded > total_size {
            tokio::fs::write(output_path, Vec::<u8>::new()).await?;
            downloaded = 0;
        }

        on_progress(downloaded, total_size);
        if downloaded == total_size {
            return Ok(StreamDownloadResult {
                downloaded,
                total: total_size,
                output_path: output_path.to_path_buf(),
            });
        }

        while downloaded < total_size {
            self.wait_if_paused(control).await?;
            self.ensure_not_cancelled(control)?;

            let start = downloaded;
            let end = start
                .saturating_add(self.config.chunk_size as u64)
                .saturating_sub(1)
                .min(total_size.saturating_sub(1));

            let bytes = self
                .fetch_chunk_with_retry(url, start, end, control)
                .await
                .with_context(|| format!("下载分段失败: bytes={}-{}", start, end))?;

            if bytes.is_empty() {
                anyhow::bail!("服务器返回空分段: bytes={}-{}", start, end);
            }

            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(output_path)
                .await?;
            file.write_all(&bytes).await?;
            file.flush().await?;

            downloaded = downloaded.saturating_add(bytes.len() as u64).min(total_size);
            on_progress(downloaded, total_size);
        }

        let final_size = self.local_file_size(output_path).await?;
        if final_size != total_size {
            anyhow::bail!(
                "文件大小校验失败: downloaded={}, expected={}",
                final_size,
                total_size
            );
        }

        Ok(StreamDownloadResult {
            downloaded: final_size,
            total: total_size,
            output_path: output_path.to_path_buf(),
        })
    }

    async fn fetch_total_size(&self, url: &str) -> Result<u64> {
        let response = self
            .client
            .get(url)
            .header("Range", "bytes=0-0")
            .header("Referer", "https://www.bilibili.com")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header("Origin", "https://www.bilibili.com")
            .header("Sec-Fetch-Site", "same-site")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "video")
            .header("Accept", "*/*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .timeout(std::time::Duration::from_secs(self.config.timeout))
            .send()
            .await?;

        if response.status().as_u16() == 206 {
            if let Some(range) = response.headers().get("content-range") {
                let range = range.to_str().unwrap_or_default();
                if let Some(total) = parse_total_from_content_range(range) {
                    return Ok(total);
                }
            }
        }

        if response.status().is_success() {
            if let Some(content_length) = response.headers().get("content-length") {
                let size = content_length.to_str().unwrap_or("0").parse::<u64>().unwrap_or(0);
                if size > 0 {
                    return Ok(size);
                }
            }
        }

        anyhow::bail!("无法获取远端文件大小");
    }

    async fn fetch_chunk_with_retry(
        &self,
        url: &str,
        start: u64,
        end: u64,
        control: &TaskControl,
    ) -> Result<Vec<u8>> {
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=self.config.max_retry {
            self.wait_if_paused(control).await?;
            self.ensure_not_cancelled(control)?;

            match self.fetch_chunk(url, start, end).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < self.config.max_retry {
                        let delay = 2u64.pow(attempt as u32);
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("下载失败")))
    }

    async fn fetch_chunk(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        let range = format!("bytes={}-{}", start, end);
        let response = self
            .client
            .get(url)
            .header("Range", &range)
            .header("Referer", "https://www.bilibili.com")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header("Origin", "https://www.bilibili.com")
            .header("Sec-Fetch-Site", "same-site")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "video")
            .header("Accept", "*/*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .timeout(std::time::Duration::from_secs(self.config.timeout))
            .send()
            .await?;

        if response.status().as_u16() == 416 {
            anyhow::bail!("Range 416: bytes={}-{}", start, end);
        }

        if response.status().as_u16() != 206 {
            anyhow::bail!(
                "下载状态码异常: status={}, range={}",
                response.status(),
                range
            );
        }

        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    async fn local_file_size(&self, path: &Path) -> Result<u64> {
        if !path.exists() {
            return Ok(0);
        }
        Ok(tokio::fs::metadata(path).await?.len())
    }

    async fn wait_if_paused(&self, control: &TaskControl) -> Result<()> {
        while control.paused.load(Ordering::SeqCst) {
            self.ensure_not_cancelled(control)?;
            tokio::time::sleep(std::time::Duration::from_millis(PAUSE_POLL_MS)).await;
        }
        Ok(())
    }

    fn ensure_not_cancelled(&self, control: &TaskControl) -> Result<()> {
        if control.cancelled.load(Ordering::SeqCst) {
            anyhow::bail!("任务已取消");
        }
        Ok(())
    }
}

fn parse_total_from_content_range(content_range: &str) -> Option<u64> {
    content_range
        .split('/')
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
}

fn create_download_client() -> Client {
    Client::builder()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .build()
        .unwrap_or_else(|_| Client::new())
}
