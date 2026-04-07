use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub mod bilibili;
pub mod youtube;

/// 支持的视频平台
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Bilibili,
    YouTube,
}

impl Platform {
    /// 从 URL 自动检测平台
    pub fn from_url(url: &str) -> Result<Self> {
        let url_lower = url.to_lowercase();

        if url_lower.contains("youtube.com") || url_lower.contains("youtu.be") {
            Ok(Platform::YouTube)
        } else if url_lower.contains("bilibili.com") {
            Ok(Platform::Bilibili)
        } else {
            anyhow::bail!("不支持的平台: {}", url)
        }
    }

    /// 获取平台名称
    pub fn name(&self) -> &'static str {
        match self {
            Platform::Bilibili => "bilibili",
            Platform::YouTube => "youtube",
        }
    }
}

/// 视频信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInfo {
    pub platform: Platform,
    pub video_id: String,
    pub title: String,
    pub author: String,
    pub duration: Option<u64>,  // 时长（秒）
    pub description: Option<String>,
    pub thumbnail: Option<String>, // 封面 URL
}

/// 流 URL 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamUrls {
    pub video_url: String,
    pub audio_url: String,
    pub video_quality: String,
    pub audio_quality: String,
}

/// 平台客户端统一接口
pub trait PlatformClient: Send + Sync {
    /// 解析 URL，获取视频信息
    fn parse_url(&self, url: &str) -> Result<VideoInfo>;

    /// 获取视频流 URL
    fn get_stream_urls(&self, video_info: &VideoInfo, quality: i32) -> Result<StreamUrls>;

    /// 验证 cookies 有效性
    fn verify_cookies(&self) -> Result<CookiesStatus>;
}

/// Cookies 状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CookiesStatus {
    #[serde(rename = "valid")]
    Valid,
    #[serde(rename = "invalid")]
    Invalid,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "not_found")]
    NotFound,
}

/// 多平台客户端工厂
pub struct ClientFactory {
    bilibili_client: Arc<Mutex<Option<Arc<bilibili::BilibiliClientAdapter>>>>,
    youtube_client: Arc<Mutex<Option<Arc<youtube::YouTubeClient>>>>,
}

impl ClientFactory {
    pub fn new() -> Self {
        Self {
            bilibili_client: Arc::new(Mutex::new(None)),
            youtube_client: Arc::new(Mutex::new(None)),
        }
    }

    /// 获取平台客户端
    pub fn get_client(&self, platform: Platform) -> Result<Arc<dyn PlatformClient>> {
        match platform {
            Platform::Bilibili => {
                let mut client = self.bilibili_client.lock().unwrap();
                if let Some(ref c) = *client {
                    Ok(c.clone() as Arc<dyn PlatformClient>)
                } else {
                    let new_client = Arc::new(bilibili::BilibiliClientAdapter::new()?);
                    *client = Some(new_client.clone());
                    Ok(new_client as Arc<dyn PlatformClient>)
                }
            }
            Platform::YouTube => {
                let mut client = self.youtube_client.lock().unwrap();
                if let Some(ref c) = *client {
                    Ok(c.clone() as Arc<dyn PlatformClient>)
                } else {
                    let new_client = Arc::new(youtube::YouTubeClient::new()?);
                    *client = Some(new_client.clone());
                    Ok(new_client as Arc<dyn PlatformClient>)
                }
            }
        }
    }
}

impl Default for ClientFactory {
    fn default() -> Self {
        Self::new()
    }
}
