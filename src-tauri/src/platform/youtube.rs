use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;
use std::process::Command;
use crate::platform::{Platform, PlatformClient, VideoInfo, StreamUrls, CookiesStatus};

/// YouTube 客户端
pub struct YouTubeClient {
    cookies_path: Option<PathBuf>,
}

impl YouTubeClient {
    pub fn new() -> Result<Self> {
        // 检查 yt-dlp 是否安装
        Self::check_yt_dlp_installed()?;

        Ok(Self {
            cookies_path: Self::get_cookies_path(),
        })
    }

    pub fn with_cookies(mut self, cookies_path: PathBuf) -> Self {
        self.cookies_path = Some(cookies_path);
        self
    }

    /// 检查 yt-dlp 是否已安装
    fn check_yt_dlp_installed() -> Result<()> {
        let output = Command::new("yt-dlp")
            .arg("--version")
            .output();

        match output {
            Ok(_) => Ok(()),
            Err(e) => {
                // 检查是否是"未找到命令"错误
                let err_str = e.to_string().to_lowercase();
                if err_str.contains("not found") || err_str.contains("no such file") {
                    anyhow::bail!(
                        "未找到 yt-dlp 工具\n\
                        请安装：brew install yt-dlp\n\
                        或访问：https://github.com/yt-dlp/yt-dlp#installation"
                    );
                }
                anyhow::bail!("检查 yt-dlp 失败: {}", e);
            }
        }
    }

    /// 获取 YouTube cookies 存储路径
    fn get_cookies_path() -> Option<PathBuf> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap())
            .join("bilibili-downloader");

        let _ = std::fs::create_dir_all(&config_dir);

        let cookies_file = config_dir.join("youtube_cookies.txt");
        if cookies_file.exists() {
            Some(cookies_file)
        } else {
            None
        }
    }

    /// 执行 yt-dlp 命令并获取输出
    fn execute_yt_dlp(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("yt-dlp");

        // 添加 cookies 参数
        if let Some(ref cookies_path) = self.cookies_path {
            if cookies_path.exists() {
                cmd.arg("--cookies").arg(cookies_path);
            }
        }

        cmd.args(args);

        let output = cmd.output()
            .context("执行 yt-dlp 命令失败")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // 检查命令是否成功执行
        if !output.status.success() {
            anyhow::bail!("yt-dlp 执行失败: {}", stderr);
        }

        // 优先使用 stdout，如果为空则使用 stderr
        let result = if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            String::new()
        };

        Ok(result)
    }

    /// 解析 YouTube URL 获取视频信息
    pub fn parse_youtube_url(&self, url: &str) -> Result<VideoInfo> {
        // 提取视频 ID
        let video_id = self.extract_video_id(url)?;

        // 使用 yt-dlp 获取视频信息
        let output = self.execute_yt_dlp(&[
            "--print",
            "%(title)s|%(channel)s|%(duration)s|%(thumbnail)s",
            url
        ])?;

        let parts: Vec<&str> = output.split('|').collect();
        if parts.len() < 4 {
            anyhow::bail!("解析 YouTube 视频信息失败，响应格式不正确");
        }

        let title = parts.get(0).unwrap_or(&"").to_string();
        let author = parts.get(1).unwrap_or(&"").to_string();
        let duration_str = parts.get(2).unwrap_or(&"").to_string();
        let thumbnail = parts.get(3).unwrap_or(&"").to_string();

        // 解析时长（格式：HH:MM:SS 或 MM:SS）
        let duration = if !duration_str.is_empty() {
            Some(Self::parse_duration(&duration_str)?)
        } else {
            None
        };

        Ok(VideoInfo {
            platform: Platform::YouTube,
            video_id,
            title,
            author,
            duration,
            description: None,
            thumbnail: Some(thumbnail),
        })
    }

    /// 提取 YouTube 视频 ID
    fn extract_video_id(&self, url: &str) -> Result<String> {
        // 支持 youtube.com/watch?v= 和 youtu.be/ 格式
        let re = regex::Regex::new(
            r"(?:youtube\.com/watch\?v=|youtu\.be/)([a-zA-Z0-9_-]+)"
        ).unwrap();

        if let Some(caps) = re.captures(url) {
            Ok(caps[1].to_string())
        } else {
            anyhow::bail!("无法解析 YouTube URL: {}", url)
        }
    }

    /// 解析时长字符串 (HH:MM:SS 或 MM:SS)
    fn parse_duration(duration_str: &str) -> Result<u64> {
        let parts: Vec<&str> = duration_str.split(':').collect();

        let seconds = match parts.len() {
            3 => {
                // HH:MM:SS
                let hours: u64 = parts[0].parse().unwrap_or(0);
                let minutes: u64 = parts[1].parse().unwrap_or(0);
                let seconds: u64 = parts[2].parse().unwrap_or(0);
                hours * 3600 + minutes * 60 + seconds
            }
            2 => {
                // MM:SS
                let minutes: u64 = parts[0].parse().unwrap_or(0);
                let seconds: u64 = parts[1].parse().unwrap_or(0);
                minutes * 60 + seconds
            }
            1 => {
                // SS (只有秒数)
                parts[0].parse().unwrap_or(0)
            }
            _ => 0,
        };

        Ok(seconds)
    }

    /// 获取流 URL
    pub fn get_stream_urls_impl(&self, url: &str, quality: i32) -> Result<StreamUrls> {
        // 调用 yt-dlp 获取视频和音频 URL
        // 优先选择 H.264 编码（避免 AV1，确保 macOS 兼容性）
        // 格式选择器说明：
        // - bestvideo[vcodec~='h264']+bestaudio: 优先选择 H.264 视频
        // - bestvideo[vcodec~='none']+bestaudio: 回退到任何可用视频
        // - best: 最后的回退选项
        let format_selector = "bestvideo[vcodec~='h264'][ext=mp4]+bestaudio[ext=m4a]/bestvideo[vcodec~='h264']+bestaudio/best[ext=mp4]/best";

        let output = self.execute_yt_dlp(&[
            "--format", format_selector,
            "--get-url",
            url
        ])?;

        let lines: Vec<&str> = output.lines().collect();

        if lines.is_empty() {
            anyhow::bail!("获取流 URL 失败，响应为空");
        }

        // 过滤掉空行和 "NA" 行
        let valid_lines: Vec<&str> = lines
            .iter()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty() && line != "NA"
            })
            .map(|s| *s)
            .collect();

        if valid_lines.is_empty() {
            anyhow::bail!("获取流 URL 失败，无法获取有效的视频链接。可能需要登录或视频不可用");
        }

        // 如果只有一行，说明是合并的视频
        // 如果有两行，第一行是视频，第二行是音频
        let (video_url, audio_url) = if valid_lines.len() >= 2 {
            (valid_lines[0].trim().to_string(), valid_lines[1].trim().to_string())
        } else {
            // 单个文件，video_url 和 audio_url 相同
            let url = valid_lines[0].trim().to_string();
            (url.clone(), url)
        };

        Ok(StreamUrls {
            video_url,
            audio_url,
            video_quality: quality.to_string(),
            audio_quality: "audio".to_string(),
        })
    }
}

impl PlatformClient for YouTubeClient {
    fn parse_url(&self, url: &str) -> Result<VideoInfo> {
        self.parse_youtube_url(url)
    }

    fn get_stream_urls(&self, video_info: &VideoInfo, _quality: i32) -> Result<StreamUrls> {
        let url = format!("https://www.youtube.com/watch?v={}", video_info.video_id);
        self.get_stream_urls_impl(&url, 120) // YouTube 优先使用高质量
    }

    fn verify_cookies(&self) -> Result<CookiesStatus> {
        // 获取 cookies 文件路径
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap())
            .join("bilibili-downloader");

        let cookies_file = config_dir.join("youtube_cookies.txt");

        // 检查文件是否存在
        if !cookies_file.exists() {
            return Ok(CookiesStatus::NotFound);
        }

        // 验证 cookies 文件格式
        let content = std::fs::read_to_string(&cookies_file)?;
        let trimmed = content.trim();

        if trimmed.is_empty() {
            return Ok(CookiesStatus::Invalid);
        }

        // 检查是否是有效的 Netscape cookie 格式
        let lines: Vec<&str> = trimmed.lines().collect();
        let mut has_valid_entry = false;

        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Netscape cookie 格式: domain \t include_subdomains \t path \t https \t expires \t name \t value
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 7 {
                has_valid_entry = true;
                break;
            }
        }

        if !has_valid_entry {
            return Ok(CookiesStatus::Invalid);
        }

        // 创建临时文件副本，避免 yt-dlp 覆盖原文件
        let temp_cookies_file = config_dir.join("youtube_cookies_temp.txt");
        std::fs::copy(&cookies_file, &temp_cookies_file)
            .context("创建临时 cookies 文件失败")?;

        // 使用临时文件验证
        let test_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";  // 测试视频
        let output = self.execute_yt_dlp(&[
            "--cookies", &temp_cookies_file.display().to_string(),
            "--print", "%(title)s",
            test_url
        ]);

        // 清理临时文件
        let _ = std::fs::remove_file(&temp_cookies_file);

        match output {
            Ok(content) if !content.trim().is_empty() => Ok(CookiesStatus::Valid),
            Ok(_) => Ok(CookiesStatus::Expired),
            Err(_) => Ok(CookiesStatus::Expired),
        }
    }
}
