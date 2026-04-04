use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use std::collections::HashSet;

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

/// 合集识别模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CollectionMode {
    /// 仅使用结构化数据（精准）
    Strict,
    /// 结构化优先 + HTML 初始状态兜底（兼容）
    Compat,
}

impl Default for CollectionMode {
    fn default() -> Self {
        Self::Strict
    }
}

impl CollectionMode {
    pub fn from_option_str(mode: Option<&str>) -> Self {
        match mode.map(|s| s.trim().to_ascii_lowercase()) {
            Some(v) if v == "compat" => Self::Compat,
            _ => Self::Strict,
        }
    }
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

    async fn get_view_data_json(&self, bvid: &str) -> Result<serde_json::Value> {
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

        Ok(json["data"].clone())
    }

    fn parse_video_info_from_data(data: &serde_json::Value) -> Result<VideoInfo> {
        let bvid = data["bvid"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("缺少 bvid"))?
            .to_string();
        let title = data["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("缺少 title"))?
            .to_string();
        let owner = data["owner"]["name"].as_str().unwrap_or("").to_string();
        let cid = data["cid"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("缺少 cid"))?;
        let intro = data["desc"].as_str().unwrap_or("").to_string();

        let pages = data["pages"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("缺少 pages"))?
            .iter()
            .filter_map(|p| {
                Some(VideoPage {
                    page: p["page"].as_i64()? as i32,
                    cid: p["cid"].as_i64()?,
                    part: p["part"].as_str().unwrap_or("").to_string(),
                    duration: p["duration"].as_i64().unwrap_or(0) as i32,
                })
            })
            .collect::<Vec<_>>();

        if pages.is_empty() {
            anyhow::bail!("视频 pages 为空");
        }

        Ok(VideoInfo {
            bvid,
            title,
            owner,
            cid,
            pages,
            intro,
        })
    }

    /// 获取视频信息
    pub async fn get_video_info(&self, bvid: &str) -> Result<VideoInfo> {
        let data = self.get_view_data_json(bvid).await?;
        Self::parse_video_info_from_data(&data)
    }

    /// 获取视频所属的合集/系列列表（结构化优先）
    pub async fn get_video_playlist_with_mode(
        &self,
        bvid: &str,
        mode: CollectionMode,
    ) -> Result<PlaylistResult> {
        let view_data = self.get_view_data_json(bvid).await?;
        let video_info = Self::parse_video_info_from_data(&view_data)?;

        // 1) 多分P优先
        if video_info.pages.len() > 1 {
            let playlist_videos: Vec<PlaylistVideo> = video_info
                .pages
                .iter()
                .map(|page| PlaylistVideo {
                    bvid: bvid.to_string(),
                    cid: page.cid,
                    title: page.part.clone(),
                    index: page.page,
                })
                .collect();

            return Ok(PlaylistResult {
                r#type: "multi_part".to_string(),
                title: video_info.title,
                videos: playlist_videos,
            });
        }

        // 2) 尝试结构化 ugc_season（仅当前 section）
        if let Some(ugc_season) = view_data["ugc_season"].as_object() {
            let ugc_season_value = serde_json::Value::Object(ugc_season.clone());
            if let Some(collection) = self
                .build_collection_from_ugc_season(
                    &ugc_season_value,
                    bvid,
                    view_data["title"].as_str().unwrap_or(&video_info.title),
                )
                .await?
            {
                return Ok(collection);
            }
        }

        // 3) compat 模式：HTML __INITIAL_STATE__ 兜底（不做全页 href 扫描）
        if mode == CollectionMode::Compat {
            if let Some(collection) = self
                .build_collection_from_html_initial_state(bvid, &video_info.title)
                .await?
            {
                return Ok(collection);
            }
        }

        // 4) 单视频
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

    async fn build_collection_from_html_initial_state(
        &self,
        bvid: &str,
        default_title: &str,
    ) -> Result<Option<PlaylistResult>> {
        let html = self.fetch_video_page_html(bvid).await?;
        let initial_state = match extract_initial_state_json(&html) {
            Some(v) => v,
            None => return Ok(None),
        };

        let ugc_season = initial_state
            .get("ugc_season")
            .or_else(|| initial_state.get("videoData").and_then(|v| v.get("ugc_season")));

        let Some(ugc_season) = ugc_season else {
            return Ok(None);
        };

        self.build_collection_from_ugc_season(ugc_season, bvid, default_title)
            .await
    }

    async fn fetch_video_page_html(&self, bvid: &str) -> Result<String> {
        let url = format!("https://www.bilibili.com/video/{}", bvid);
        let mut req = self
            .client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Accept-Encoding", "gzip, deflate, br")
            .header("Referer", "https://www.bilibili.com")
            .header("Connection", "keep-alive");

        if let Some(ref sessdata) = self.sessdata {
            req = req.header("Cookie", format!("SESSDATA={}", sessdata));
        }

        let resp = req.send().await?;
        Ok(resp.text().await?)
    }

    async fn build_collection_from_ugc_season(
        &self,
        ugc_season: &serde_json::Value,
        current_bvid: &str,
        default_title: &str,
    ) -> Result<Option<PlaylistResult>> {
        let sections = ugc_season
            .get("sections")
            .and_then(|v| v.as_array())
            .filter(|arr| !arr.is_empty());

        let Some(sections) = sections else {
            return Ok(None);
        };

        let current_section = sections
            .iter()
            .find(|section| section_contains_bvid(section, current_bvid));

        let Some(current_section) = current_section else {
            return Ok(None);
        };

        let episodes = current_section
            .get("episodes")
            .and_then(|v| v.as_array())
            .filter(|arr| !arr.is_empty());

        let Some(episodes) = episodes else {
            return Ok(None);
        };

        let mut seen = HashSet::new();
        let mut videos = Vec::new();

        for episode in episodes {
            let Some(episode_bvid) = extract_episode_bvid(episode) else {
                continue;
            };
            if !seen.insert(episode_bvid.clone()) {
                continue;
            }

            // cid 优先级: episode.page.cid -> episode.cid -> get_video_info 回查
            let cid = match extract_episode_cid(episode) {
                Some(v) => v,
                None => match self.get_video_info(&episode_bvid).await {
                    Ok(info) => info.cid,
                    Err(_) => continue,
                },
            };

            let title = extract_episode_title(episode)
                .unwrap_or_else(|| format!("选集 {}", videos.len() + 1));

            videos.push(PlaylistVideo {
                bvid: episode_bvid,
                cid,
                title,
                index: (videos.len() + 1) as i32,
            });
        }

        if videos.len() <= 1 {
            return Ok(None);
        }

        let collection_title = ugc_season
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(default_title)
            .to_string();

        Ok(Some(PlaylistResult {
            r#type: "collection".to_string(),
            title: collection_title,
            videos,
        }))
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

fn extract_initial_state_json(html: &str) -> Option<serde_json::Value> {
    let marker = "window.__INITIAL_STATE__=";
    let start = html.find(marker)? + marker.len();
    let tail = &html[start..];

    let end = tail
        .find(";(function")
        .or_else(|| tail.find(";</script>"))
        .or_else(|| tail.find('\n'))?;

    let json_text = tail[..end].trim();
    serde_json::from_str::<serde_json::Value>(json_text).ok()
}

fn section_contains_bvid(section: &serde_json::Value, bvid: &str) -> bool {
    section
        .get("episodes")
        .and_then(|v| v.as_array())
        .map(|episodes| {
            episodes
                .iter()
                .any(|episode| extract_episode_bvid(episode).as_deref() == Some(bvid))
        })
        .unwrap_or(false)
}

fn extract_episode_bvid(episode: &serde_json::Value) -> Option<String> {
    episode
        .get("bvid")
        .and_then(|v| v.as_str())
        .or_else(|| episode.get("arc").and_then(|v| v.get("bvid")).and_then(|v| v.as_str()))
        .map(|v| v.to_string())
}

fn extract_episode_title(episode: &serde_json::Value) -> Option<String> {
    episode
        .get("title")
        .and_then(|v| v.as_str())
        .or_else(|| episode.get("arc").and_then(|v| v.get("title")).and_then(|v| v.as_str()))
        .map(|v| v.to_string())
}

fn extract_episode_cid(episode: &serde_json::Value) -> Option<i64> {
    episode
        .get("page")
        .and_then(|v| v.get("cid"))
        .and_then(|v| v.as_i64())
        .or_else(|| episode.get("cid").and_then(|v| v.as_i64()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_extract_initial_state_json() {
        let html = r#"<script>window.__INITIAL_STATE__={"videoData":{"ugc_season":{"title":"合集A"}}};(function(){})</script>"#;
        let parsed = extract_initial_state_json(html).expect("should parse");
        assert_eq!(
            parsed["videoData"]["ugc_season"]["title"].as_str(),
            Some("合集A")
        );
    }

    #[test]
    fn should_match_section_by_current_bvid() {
        let section = serde_json::json!({
            "episodes": [
                { "bvid": "BV111" },
                { "arc": { "bvid": "BV222" } }
            ]
        });
        assert!(section_contains_bvid(&section, "BV222"));
        assert!(!section_contains_bvid(&section, "BV333"));
    }

    #[test]
    fn should_extract_episode_cid_by_priority() {
        let episode = serde_json::json!({
            "cid": 100,
            "page": { "cid": 200 }
        });
        assert_eq!(extract_episode_cid(&episode), Some(200));

        let episode2 = serde_json::json!({ "cid": 300 });
        assert_eq!(extract_episode_cid(&episode2), Some(300));
    }

    #[test]
    fn should_extract_episode_bvid_and_title() {
        let episode = serde_json::json!({
            "arc": { "bvid": "BVabc", "title": "标题1" }
        });
        assert_eq!(extract_episode_bvid(&episode).as_deref(), Some("BVabc"));
        assert_eq!(extract_episode_title(&episode).as_deref(), Some("标题1"));
    }
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
    /// 质量降级列表（从高到低）
    const QUALITY_FALLBACK: &[i32] = &[120, 116, 112, 80, 64, 32];

    /// 获取视频播放URL（支持质量自动降级）
    pub async fn get_play_url(&self, bvid: &str, cid: i64, quality: i32) -> Result<PlayUrlResult> {
        // 先尝试请求的质量
        match self.try_get_play_url(bvid, cid, quality).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // 如果是 403 错误或没有视频流，尝试降级
                if e.to_string().contains("403") || e.to_string().contains("未找到视频流") {
                    let fallback_qualities: Vec<i32> = Self::QUALITY_FALLBACK
                        .iter()
                        .filter(|&&q| q < quality)
                        .copied()
                        .collect();

                    for fallback_quality in fallback_qualities {
                        match self.try_get_play_url(bvid, cid, fallback_quality).await {
                            Ok(result) => {
                                eprintln!("质量 {} 不可用，已自动降级到 {}", quality, fallback_quality);
                                return Ok(result);
                            }
                            Err(_) => continue,
                        }
                    }

                    anyhow::bail!("所有质量等级都无法下载: {}", e);
                }
                return Err(e);
            }
        }
    }

    /// 尝试获取指定质量的播放URL
    async fn try_get_play_url(&self, bvid: &str, cid: i64, quality: i32) -> Result<PlayUrlResult> {
        let url = format!(
            "https://api.bilibili.com/x/player/playurl?bvid={}&cid={}&qn={}&fnval=16&fourk=1",
            bvid, cid, quality
        );

        let mut req = self.client.get(&url)
            .header("Referer", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .header("Origin", "https://www.bilibili.com")
            .header("Sec-Fetch-Site", "same-site")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "empty")
            .header("Accept", "application/json, text/plain, */*");

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

        // 尝试多个视频流，找到可用的
        if let Some(video_array) = dash["video"].as_array() {
            if !video_array.is_empty() {
                let mut video_url: Option<String> = None;

                for (index, video) in video_array.iter().enumerate() {
                    if let Some(url) = video["baseUrl"].as_str() {
                        let url_str = url.to_string();
                        eprintln!("尝试视频流 {}/{}: {}", index + 1, video_array.len(),
                            url_str.split('?').next().unwrap_or(&url_str));

                        // 验证 URL 可用性
                        match self.validate_url(&url_str).await {
                            true => {
                                eprintln!("✓ 视频流 {} 验证通过", index + 1);
                                video_url = Some(url_str);
                                break;
                            }
                            false => {
                                eprintln!("⚠ 视频流 {} 验证失败，尝试下一个", index + 1);
                            }
                        }
                    }
                }

                // 如果所有视频流都验证失败，使用第一个
                let video_url = video_url.or_else(|| {
                    video_array.first()
                        .and_then(|v| v["baseUrl"].as_str())
                        .map(|s| {
                            eprintln!("⚠ 所有视频流验证失败，使用第一个视频流");
                            s.to_string()
                        })
                });

                if let Some(video_url) = video_url {

            if let Some(audio_array) = dash["audio"].as_array() {
                if !audio_array.is_empty() {
                    // 尝试多个音频流，找到可用的
                    for (index, audio) in audio_array.iter().enumerate() {
                        if let Some(audio_url) = audio["baseUrl"].as_str() {
                            let audio_url_str = audio_url.to_string();
                            eprintln!("尝试音频流 {}/{}: {}", index + 1, audio_array.len(),
                                audio_url_str.split('?').next().unwrap_or(&audio_url_str));

                            // 验证 URL 可用性
                            match self.validate_url(&audio_url_str).await {
                                true => {
                                    eprintln!("✓ 音频流 {} 验证通过", index + 1);

                                    // 不预先获取文件大小，改为在下载时动态获取
                                    // 不同 CDN 节点可能返回不同大小，预先获取不准确
                                    let video_size = 0u64;
                                    let audio_size = 0u64;

                                    return Ok(PlayUrlResult {
                                        video_url,
                                        audio_url: audio_url_str,
                                        video_quality: data["quality"].as_i64().unwrap() as i32,
                                        video_size,
                                        audio_size,
                                    });
                                }
                                false => {
                                    eprintln!("⚠ 音频流 {} 验证失败（{}），尝试下一个",
                                        index + 1,
                                        audio_url_str.split('/').nth(2).unwrap_or("unknown")
                                    );
                                    // 继续尝试下一个音频流
                                }
                            }
                        }
                    }

                    // 如果所有音频流都验证失败，使用第一个作为最后的尝试
                    if let Some(first_audio) = audio_array.first() {
                        if let Some(first_url) = first_audio["baseUrl"].as_str() {
                            eprintln!("⚠ 所有音频流验证失败，使用第一个音频流作为最后的尝试");
                            let video_size = 0u64;
                            let audio_size = 0u64;

                            return Ok(PlayUrlResult {
                                video_url,
                                audio_url: first_url.to_string(),
                                video_quality: data["quality"].as_i64().unwrap() as i32,
                                video_size,
                                audio_size,
                            });
                        }
                    }

                    anyhow::bail!("未找到可用的音频流")
                        }
                    }
                }
            }
        }

        anyhow::bail!("未找到视频流")
    }

    /// 验证 URL 是否可用（发送 HEAD 请求检查）
    async fn validate_url(&self, url: &str) -> bool {
        let response = self.client
            .head(url)
            .header("Referer", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(10))
            .send();

        match response.await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                status == 200 || status == 206
            }
            Err(_) => false,
        }
    }

}
