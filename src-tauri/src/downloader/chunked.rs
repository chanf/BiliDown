use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinSet;

use crate::downloader::{DownloadConfig, TaskControl};

const PAUSE_POLL_MS: u64 = 200;

pub struct StreamDownloadResult {
    pub downloaded: u64,
    pub total: u64,
    pub output_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct ChunkPlan {
    index: usize,
    start: u64,
    end: u64,
    len: u64,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct ChunkJob {
    plan: ChunkPlan,
    existing: u64,
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
        let chunk_dir = chunk_dir_from_output(output_path);
        tokio::fs::create_dir_all(&chunk_dir).await?;

        // 动态分块策略：根据文件大小调整分块大小
        // 小文件: 1MB, 中文件: 5MB, 大文件: 10MB
        let dynamic_chunk_size = if total_size < 50 * 1024 * 1024 {
            1024 * 1024  // 1MB
        } else if total_size < 200 * 1024 * 1024 {
            5 * 1024 * 1024  // 5MB
        } else {
            10 * 1024 * 1024  // 10MB
        };
        let plans = build_chunk_plans(total_size, dynamic_chunk_size, &chunk_dir);
        if plans.is_empty() {
            anyhow::bail!("分块规划失败，文件大小无效");
        }

        let mut downloaded = 0u64;
        let mut jobs = Vec::new();

        for plan in &plans {
            let existing = inspect_existing_chunk(plan).await?;
            downloaded = downloaded.saturating_add(existing);

            if existing < plan.len {
                jobs.push(ChunkJob {
                    plan: plan.clone(),
                    existing,
                });
            }
        }

        on_progress(downloaded, total_size);

        if jobs.is_empty() {
            return Ok(StreamDownloadResult {
                downloaded: total_size,
                total: total_size,
                output_paths: plans.into_iter().map(|p| p.path).collect(),
            });
        }

        // 动态并发策略：根据文件大小调整并发数
        // 小文件(<50MB): 2并发, 中文件(50-200MB): 4并发, 大文件(>200MB): 8并发
        let dynamic_concurrency = if total_size < 50 * 1024 * 1024 {
            2
        } else if total_size < 200 * 1024 * 1024 {
            4
        } else {
            8
        };
        let concurrency = self.config.concurrent_connections.max(dynamic_concurrency).min(16); // 最大不超过16
        let mut queue: VecDeque<ChunkJob> = VecDeque::from(jobs);
        let mut join_set = JoinSet::new();
        let client = self.client.clone();
        let config = self.config.clone();
        let url_owned = url.to_string();

        for _ in 0..concurrency.min(queue.len()) {
            if let Some(job) = queue.pop_front() {
                let client = client.clone();
                let config = config.clone();
                let url = url_owned.clone();
                let control = control.clone();
                join_set.spawn(async move { download_chunk_job(client, config, url, job, control).await });
            }
        }

        while let Some(join_result) = join_set.join_next().await {
            let newly_downloaded = join_result
                .map_err(|e| anyhow::anyhow!("并发下载任务异常退出: {}", e))??;
            downloaded = downloaded.saturating_add(newly_downloaded).min(total_size);
            on_progress(downloaded, total_size);

            if let Some(job) = queue.pop_front() {
                let client = client.clone();
                let config = config.clone();
                let url = url_owned.clone();
                let control = control.clone();
                join_set.spawn(async move { download_chunk_job(client, config, url, job, control).await });
            }
        }

        for plan in &plans {
            let size = local_file_size(&plan.path).await?;
            if size != plan.len {
                anyhow::bail!(
                    "分块校验失败: index={}, got={}, expected={}",
                    plan.index,
                    size,
                    plan.len
                );
            }
        }

        on_progress(total_size, total_size);

        Ok(StreamDownloadResult {
            downloaded: total_size,
            total: total_size,
            output_paths: plans.into_iter().map(|p| p.path).collect(),
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
}

fn build_chunk_plans(total_size: u64, chunk_size: u64, chunk_dir: &Path) -> Vec<ChunkPlan> {
    let mut plans = Vec::new();
    let mut index = 0usize;
    let mut start = 0u64;

    while start < total_size {
        let end = start
            .saturating_add(chunk_size)
            .saturating_sub(1)
            .min(total_size.saturating_sub(1));
        plans.push(ChunkPlan {
            index,
            start,
            end,
            len: end.saturating_sub(start).saturating_add(1),
            path: chunk_dir.join(format!("{:06}.part", index)),
        });
        index = index.saturating_add(1);
        start = end.saturating_add(1);
    }

    plans
}

fn chunk_dir_from_output(output_path: &Path) -> PathBuf {
    let base_name = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("stream");
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{}.chunks", base_name))
}

async fn inspect_existing_chunk(plan: &ChunkPlan) -> Result<u64> {
    let size = local_file_size(&plan.path).await?;
    if size <= plan.len {
        return Ok(size);
    }

    tokio::fs::remove_file(&plan.path).await.ok();
    Ok(0)
}

async fn download_chunk_job(
    client: Client,
    config: DownloadConfig,
    url: String,
    job: ChunkJob,
    control: TaskControl,
) -> Result<u64> {
    if job.existing >= job.plan.len {
        return Ok(0);
    }

    wait_if_paused(&control).await?;
    ensure_not_cancelled(&control)?;

    let range_start = job.plan.start.saturating_add(job.existing);
    let range_end = job.plan.end;
    let expected = job.plan.len.saturating_sub(job.existing);

    let bytes = fetch_chunk_with_retry(&client, &config, &url, range_start, range_end, &control)
        .await
        .with_context(|| {
            format!(
                "下载分块失败: index={}, bytes={}-{}, expected={} bytes\nURL: {}",
                job.plan.index, range_start, range_end, expected,
                url.chars().take(100).collect::<String>()
            )
        })?;

    if bytes.is_empty() {
        anyhow::bail!(
            "服务器返回空分块: index={}, range={}-{}\nURL: {}",
            job.plan.index, range_start, range_end,
            url.chars().take(100).collect::<String>()
        );
    }

    if bytes.len() as u64 != expected {
        anyhow::bail!(
            "分块长度不匹配: index={}, got={}, expected={}\nURL: {}",
            job.plan.index,
            bytes.len(),
            expected,
            url.chars().take(100).collect::<String>()
        );
    }

    if let Some(parent) = job.plan.path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&job.plan.path)
        .await
        .with_context(|| format!("无法创建分块文件: {:?}", job.plan.path))?;
    file.write_all(&bytes).await?;
    file.flush().await?;

    let final_size = local_file_size(&job.plan.path).await?;
    if final_size != job.plan.len {
        anyhow::bail!(
            "分块写入校验失败: index={}, got={}, expected={}, path={:?}",
            job.plan.index,
            final_size,
            job.plan.len,
            job.plan.path
        );
    }

    Ok(expected)
}

async fn fetch_chunk_with_retry(
    client: &Client,
    config: &DownloadConfig,
    url: &str,
    start: u64,
    end: u64,
    control: &TaskControl,
) -> Result<Vec<u8>> {
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..=config.max_retry {
        wait_if_paused(control).await?;
        ensure_not_cancelled(control)?;

        match fetch_chunk(client, config, url, start, end).await {
            Ok(bytes) => {
                if attempt > 0 {
                    eprintln!("✓ 分块下载重试成功: range={}-{}, 尝试次数={}", start, end, attempt + 1);
                }
                return Ok(bytes);
            }
            Err(err) => {
                last_error = Some(err);
                if attempt < config.max_retry {
                    let delay = 2u64.pow(attempt as u32);
                    eprintln!(
                        "⚠ 分块下载失败，将在 {} 秒后重试 ({}/{}): range={}-{}, 错误: {}",
                        delay,
                        attempt + 1,
                        config.max_retry + 1,
                        start,
                        end,
                        last_error.as_ref().map(|e| e.to_string().chars().take(100).collect::<String>()).unwrap_or_default()
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("下载失败，已重试 {} 次", config.max_retry + 1)))
}

async fn fetch_chunk(
    client: &Client,
    config: &DownloadConfig,
    url: &str,
    start: u64,
    end: u64,
) -> Result<Vec<u8>> {
    let range = format!("bytes={}-{}", start, end);
    let response = client
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
        .timeout(std::time::Duration::from_secs(config.timeout))
        .send()
        .await?;

    if response.status().as_u16() == 416 {
        anyhow::bail!(
            "Range请求不可满足: bytes={}-{}, 请尝试删除缓存重新下载\nURL: {}",
            start, end,
            url.chars().take(100).collect::<String>()
        );
    }

    if response.status().as_u16() != 206 {
        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "下载状态码异常: status={}, range={}\n响应: {}\nURL: {}",
            status,
            range,
            response_text.chars().take(200).collect::<String>(),
            url.chars().take(100).collect::<String>()
        );
    }

    let bytes = response.bytes().await.with_context(|| {
        format!(
            "读取响应数据失败: range={}-{}\nURL: {}",
            start, end,
            url.chars().take(100).collect::<String>()
        )
    })?;

    Ok(bytes.to_vec())
}

async fn local_file_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(tokio::fs::metadata(path).await?.len())
}

async fn wait_if_paused(control: &TaskControl) -> Result<()> {
    while control.paused.load(Ordering::SeqCst) {
        ensure_not_cancelled(control)?;
        tokio::time::sleep(std::time::Duration::from_millis(PAUSE_POLL_MS)).await;
    }
    Ok(())
}

fn ensure_not_cancelled(control: &TaskControl) -> Result<()> {
    if control.cancelled.load(Ordering::SeqCst) {
        anyhow::bail!("任务已取消");
    }
    Ok(())
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
        .pool_max_idle_per_host(10)      // 增加 HTTP 连接池大小
        .pool_idle_timeout(std::time::Duration::from_secs(90))  // keep-alive 90秒
        .connect_timeout(std::time::Duration::from_secs(10))    // 连接超时 10秒
        .http2_keep_alive_interval(std::time::Duration::from_secs(30))  // HTTP/2 keep-alive
        .http2_keep_alive_timeout(std::time::Duration::from_secs(10))
        .tcp_keepalive(std::time::Duration::from_secs(60))      // TCP keep-alive
        .tcp_nodelay(true)                                       // 禁用 Nagle 算法
        .build()
        .unwrap_or_else(|_| Client::new())
}
