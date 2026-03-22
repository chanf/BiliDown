use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::Result;

/// B 站 URL 类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BilibiliUrlType {
    SingleVideo { bvid: String },
}

/// B 站视频信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInfo {
    pub bvid: String,
    pub title: String,
    pub owner: String,
    pub cid: i64,
    pub pages: Vec<VideoPage>,
    pub intro: String,
}

/// 分 P 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoPage {
    pub page: i32,
    pub cid: i64,
    pub part: String,
    pub duration: i32,
}

/// 合集/系列视频列表
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistVideo {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub index: i32,
}

/// B 站客户端
pub struct BilibiliClient {
    client: Client,
    sessdata: Option<String>,
}

impl BilibiliClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            sessdata: None,
        }
    }

    pub fn with_sessdata(mut self, sessdata: String) -> Self {
        self.sessdata = Some(sessdata);
        self
    }

    /// 解析 B 站 URL
    pub fn parse_url(url: &str) -> Result<(BilibiliUrlType, Option<String>)> {
        let re = regex::Regex::new(r"bilibili\.com/video/(BV[\w]+)").unwrap();
        if let Some(caps) = re.captures(url) {
            return Ok((BilibiliUrlType::SingleVideo {
                bvid: caps[1].to_string(),
            }, Some(caps[1].to_string())));
        }

        anyhow::bail!("无法识别的 B 站 URL");
    }

    /// 获取视频信息
    pub async fn get_video_info(&self, bvid: &str) -> Result<VideoInfo> {
        let url = format!("https://api.bilibili.com/x/web-interface/view?bvid={}", bvid);
        let mut req = self.client.get(&url);

        if let Some(ref sessdata) = self.sessdata {
            req = req.header("Cookie", format!("SESSDATA={}", sessdata));
        }

        let resp = req.send().await?;
        let json: serde_json::Value = resp.json().await?;

        if json["code"] != 0 {
            anyhow::bail!("获取视频信息失败: {}", json["message"]);
        }

        let data = &json["data"];
        Ok(VideoInfo {
            bvid: data["bvid"].as_str().unwrap().to_string(),
            title: data["title"].as_str().unwrap().to_string(),
            owner: data["owner"]["name"].as_str().unwrap().to_string(),
            cid: data["cid"].as_i64().unwrap(),
            pages: data["pages"].as_array()
                .unwrap()
                .iter()
                .map(|p| VideoPage {
                    page: p["page"].as_i64().unwrap() as i32,
                    cid: p["cid"].as_i64().unwrap(),
                    part: p["part"].as_str().unwrap().to_string(),
                    duration: p["duration"].as_i64().unwrap() as i32,
                })
                .collect(),
            intro: data["desc"].as_str().unwrap_or("").to_string(),
        })
    }

    /// 获取视频所属的合集/系列列表（优先使用 API 的 pages 信息）
    pub async fn get_video_playlist(&self, bvid: &str) -> Result<PlaylistResult> {
        // 首先获取视频信息，检查是否有分P
        let video_info = self.get_video_info(bvid).await?;

        // 如果有多个分P，直接使用分P信息
        if video_info.pages.len() > 1 {
            let playlist_videos: Vec<PlaylistVideo> = video_info.pages.iter().map(|page| {
                PlaylistVideo {
                    bvid: bvid.to_string(),
                    cid: page.cid,
                    title: page.part.clone(),
                    index: page.page,
                }
            }).collect();

            return Ok(PlaylistResult {
                r#type: "multi_part".to_string(),
                title: video_info.title,
                videos: playlist_videos,
            });
        }

        // 单个分P，尝试从 HTML 中提取合集链接
        let url = format!("https://www.bilibili.com/video/{}", bvid);
        let mut req = self.client.get(&url)
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Accept-Encoding", "gzip, deflate, br")
            .header("Referer", "https://www.bilibili.com")
            .header("Connection", "keep-alive");

        if let Some(ref sessdata) = self.sessdata {
            req = req.header("Cookie", format!("SESSDATA={}", sessdata));
        }

        let resp = req.send().await?;
        let html = resp.text().await?;

        // 从 HTML 中提取选集列表
        let playlist_bvids = parse_playlist_from_html(&html);

        if playlist_bvids.len() > 1 {
            // 找到合集，获取每个视频的标题
            let mut playlist_videos = Vec::new();
            for (i, pbvid) in playlist_bvids.iter().enumerate() {
                match self.get_video_info(pbvid).await {
                    Ok(info) => {
                        playlist_videos.push(PlaylistVideo {
                            bvid: pbvid.clone(),
                            cid: info.cid,
                            title: info.title,
                            index: (i + 1) as i32,
                        });
                    }
                    Err(_) => {
                        // 如果获取失败，使用默认标题
                        playlist_videos.push(PlaylistVideo {
                            bvid: pbvid.clone(),
                            cid: 0,
                            title: format!("选集 {}", i + 1),
                            index: (i + 1) as i32,
                        });
                    }
                }
            }

            let collection_title = extract_collection_title_from_html(&html);
            return Ok(PlaylistResult {
                r#type: "collection".to_string(),
                title: collection_title,
                videos: playlist_videos,
            });
        }

        // 单个视频
        Ok(PlaylistResult {
            r#type: "single".to_string(),
            title: video_info.title.clone(),
            videos: vec![PlaylistVideo {
                bvid: video_info.bvid,
                cid: video_info.cid,
                title: video_info.title,
                index: 1,
            }],
        })
    }
}

impl Default for BilibiliClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistResult {
    pub r#type: String,
    pub title: String,
    pub videos: Vec<PlaylistVideo>,
}

/// 从 HTML 中提取选集列表
fn parse_playlist_from_html(html: &str) -> Vec<String> {
    let mut bv_ids = vec![];
    let mut seen = std::collections::HashSet::new();

    // 解析 `<a>` 标签中的 href，匹配 BV 开头的 ID
    let re = regex::Regex::new(r#"href="/video/(BV[a-zA-Z0-9]+)"#).unwrap();
    for caps in re.captures_iter(html) {
        if let Some(bvid) = caps.get(1) {
            let bvid_str = bvid.as_str().to_string();
            if !seen.contains(&bvid_str) {
                seen.insert(bvid_str.clone());
                bv_ids.push(bvid_str);
            }
        }
    }

    // 限制数量，避免太多
    bv_ids.truncate(100);
    bv_ids
}

/// 从 HTML 中提取合集标题
fn extract_collection_title_from_html(html: &str) -> String {
    // 尝试从 title 标签提取
    let re = regex::Regex::new(r#"<title[^>]*>([^<]+)</title>"#).unwrap();
    if let Some(caps) = re.captures(html) {
        if let Some(title_match) = caps.get(1) {
            let title = title_match.as_str();
            // 格式: "主标题|副标题" 或 "主标题 - 副标题"
            if let Some(pos) = title.find('|') {
                return title[..pos].trim().to_string();
            }
            if let Some(pos) = title.find('-') {
                return title[..pos].trim().to_string();
            }
            return title.trim().to_string();
        }
    }

    "视频合集".to_string()
}

/// 播放URL结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayUrlResult {
    pub video_url: String,
    pub audio_url: String,
    pub video_quality: i32,
    pub video_size: u64,
    pub audio_size: u64,
}

impl BilibiliClient {
    /// 获取视频播放URL
    pub async fn get_play_url(&self, bvid: &str, cid: i64, quality: i32) -> Result<PlayUrlResult> {
        let url = format!(
            "https://api.bilibili.com/x/player/playurl?bvid={}&cid={}&qn={}&fnval=16&fourk=1",
            bvid, cid, quality
        );

        let mut req = self.client.get(&url);

        if let Some(ref sessdata) = self.sessdata {
            req = req.header("Cookie", format!("SESSDATA={}", sessdata));
        }

        let resp = req.send().await?;
        let json: serde_json::Value = resp.json().await?;

        if json["code"] != 0 {
            anyhow::bail!("获取播放URL失败: {}", json["message"]);
        }

        let data = &json["data"];
        let dash = &data["dash"];

        if let Some(video) = dash["video"].as_array().and_then(|v| v.first()) {
            let video_url = video["baseUrl"].as_str().unwrap().to_string();
            let video_bandwidth = video["bandwidth"].as_i64().unwrap_or(0) as u64;

            if let Some(audio) = dash["audio"].as_array().and_then(|a| a.first()) {
                let audio_url = audio["baseUrl"].as_str().unwrap().to_string();
                let audio_bandwidth = audio["bandwidth"].as_i64().unwrap_or(0) as u64;

                return Ok(PlayUrlResult {
                    video_url,
                    audio_url,
                    video_quality: data["quality"].as_i64().unwrap() as i32,
                    video_size: video_bandwidth,
                    audio_size: audio_bandwidth,
                });
            }
        }

        anyhow::bail!("未找到视频流");
    }
}
