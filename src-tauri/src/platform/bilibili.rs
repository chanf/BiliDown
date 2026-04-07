use anyhow::Result;
use std::sync::Arc;
use crate::platform::{Platform, PlatformClient, VideoInfo, StreamUrls, CookiesStatus};
use crate::bilibili::{BilibiliClient as InnerBilibiliClient, CollectionMode};

/// Bilibili 平台客户端适配器
pub struct BilibiliClientAdapter {
    client: Arc<InnerBilibiliClient>,
}

impl BilibiliClientAdapter {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Arc::new(InnerBilibiliClient::new()),
        })
    }

    /// 在当前异步上下文中阻塞执行异步操作
    fn block_on_async<F, R>(&self, f: F) -> R
    where
        F: std::future::Future<Output = R>,
    {
        // 使用 tokio 的 Handle 在当前运行时中执行
        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| {
                // 如果没有当前运行时，创建一个新的
                tokio::runtime::Runtime::new().unwrap().handle().clone()
            });

        handle.block_on(f)
    }
}

impl PlatformClient for BilibiliClientAdapter {
    fn parse_url(&self, url: &str) -> Result<VideoInfo> {
        // 使用静态方法解析 URL
        let (_url_type, bvid) = InnerBilibiliClient::parse_url(url)?;
        let bvid = bvid.ok_or_else(|| anyhow::anyhow!("无法解析视频 ID"))?;

        // 使用当前 runtime 执行异步方法
        let video_data = self.block_on_async(self.client.get_video_info(&bvid))?;

        // 计算总时长（所有分P时长之和）
        let total_duration: i32 = video_data.pages.iter().map(|p| p.duration).sum();

        Ok(VideoInfo {
            platform: Platform::Bilibili,
            video_id: bvid,
            title: video_data.title,
            author: video_data.owner,
            duration: Some(total_duration as u64),
            description: Some(video_data.intro),
            thumbnail: None, // B站API未返回封面
        })
    }

    fn get_stream_urls(&self, video_info: &VideoInfo, quality: i32) -> Result<StreamUrls> {
        // 获取播放列表以找到对应的 CID
        let playlist_data = self.block_on_async(
            self.client.get_video_playlist_with_mode(&video_info.video_id, CollectionMode::Strict)
        )?;

        // 获取第一个视频的流信息
        if playlist_data.videos.is_empty() {
            anyhow::bail!("播放列表为空");
        }

        let first_video = &playlist_data.videos[0];
        let cid = first_video.cid;

        // 获取播放 URL
        let play_url = self.block_on_async(
            self.client.get_play_url(&video_info.video_id, cid, quality)
        )?;

        Ok(StreamUrls {
            video_url: play_url.video_url,
            audio_url: play_url.audio_url,
            video_quality: quality.to_string(),
            audio_quality: "audio".to_string(),
        })
    }

    fn verify_cookies(&self) -> Result<CookiesStatus> {
        // 检查配置目录中的 sessdata.txt 文件
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap())
            .join("bilibili-downloader");

        let sessdata_file = config_dir.join("sessdata.txt");

        if !sessdata_file.exists() {
            return Ok(CookiesStatus::NotFound);
        }

        let content = std::fs::read_to_string(&sessdata_file)?;
        let trimmed = content.trim();

        if trimmed.is_empty() {
            return Ok(CookiesStatus::Invalid);
        }

        Ok(CookiesStatus::Valid)
    }
}
