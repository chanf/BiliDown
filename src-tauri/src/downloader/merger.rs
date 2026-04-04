use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Sender;

pub struct VideoMerger {
    ffmpeg_path: String,
}

impl VideoMerger {
    pub fn new(ffmpeg_path: String) -> Self {
        Self { ffmpeg_path }
    }

    pub async fn merge(
        &self,
        video_paths: &[std::path::PathBuf],
        audio_paths: &[std::path::PathBuf],
        output_path: &Path,
        progress_tx: Sender<f32>,
    ) -> Result<()> {
        eprintln!("=== 开始合并流程 ===");
        eprintln!("输出文件: {}", output_path.display());

        let temp_dir = output_path.parent().unwrap_or_else(|| Path::new("."));
        eprintln!("临时目录: {}", temp_dir.display());

        let (video_combined, audio_combined) = Self::build_temp_stream_paths(temp_dir, output_path);

        eprintln!("合并视频分块...");
        self.merge_chunks(video_paths, &video_combined, "video").await?;
        eprintln!("视频合并完成: {}", video_combined.display());

        eprintln!("合并音频分块...");
        self.merge_chunks(audio_paths, &audio_combined, "audio").await?;
        eprintln!("音频合并完成: {}", audio_combined.display());

        eprintln!("合并音视频流...");
        self.merge_streams(&video_combined, &audio_combined, output_path).await?;
        eprintln!("音视频合并完成: {}", output_path.display());

        // 验证输出文件存在
        if output_path.exists() {
            let size = tokio::fs::metadata(output_path).await?.len();
            eprintln!("最终文件大小: {} 字节 ({:.2} MB)", size, size as f64 / 1024.0 / 1024.0);
        } else {
            eprintln!("警告: 输出文件不存在!");
        }

        let _ = progress_tx.send(1.0).await;

        tokio::fs::remove_file(&video_combined).await.ok();
        tokio::fs::remove_file(&audio_combined).await.ok();

        Ok(())
    }

    fn build_temp_stream_paths(temp_dir: &Path, output_path: &Path) -> (PathBuf, PathBuf) {
        let file_stem = output_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = format!("{}_{}_{}", file_stem, std::process::id(), timestamp);

        (
            temp_dir.join(format!("video_combined_{}.mp4", unique)),
            temp_dir.join(format!("audio_combined_{}.m4a", unique)),
        )
    }

    async fn merge_chunks(
        &self,
        chunks: &[std::path::PathBuf],
        output_path: &Path,
        stream_type: &str,
    ) -> Result<()> {
        if chunks.is_empty() {
            anyhow::bail!("{}分块列表为空", stream_type);
        }

        if chunks.len() == 1 {
            tokio::fs::copy(&chunks[0], output_path).await?;
            return Ok(());
        }

        // 验证所有分块文件存在并计算总大小
        let mut total_size = 0u64;
        for chunk in chunks {
            if !chunk.exists() {
                anyhow::bail!("分块文件不存在: {}", chunk.display());
            }
            match tokio::fs::metadata(chunk).await {
                Ok(metadata) => {
                    total_size += metadata.len();
                }
                Err(e) => {
                    anyhow::bail!("无法获取分块文件大小: {}", e);
                }
            }
        }

        eprintln!("合并{}: {} 个分块, 总大小 {} 字节 ({:.2} MB)",
                 stream_type, chunks.len(), total_size, total_size as f64 / 1024.0 / 1024.0);

        // 分块是按 HTTP Range 拆分的原始字节，必须按顺序做二进制拼接
        // 不能使用 ffmpeg concat（要求输入是独立且可解复用的媒体文件）
        let mut output_file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(output_path)
            .await?;

        for chunk in chunks {
            let mut input_file = tokio::fs::File::open(chunk).await?;
            tokio::io::copy(&mut input_file, &mut output_file).await?;
        }

        output_file.flush().await?;

        let merged_size = tokio::fs::metadata(output_path).await?.len();
        if merged_size != total_size {
            anyhow::bail!(
                "合并{}分块后大小不一致: got={}, expected={}",
                stream_type,
                merged_size,
                total_size
            );
        }

        Ok(())
    }

    async fn merge_streams(
        &self,
        video_path: &Path,
        audio_path: &Path,
        output_path: &Path,
    ) -> Result<()> {
        // 检测是否需要 macOS 兼容性处理
        let is_macos = std::env::consts::OS == "macos";
        let needs_hevc_fix = is_macos && self.is_hevc_video(video_path).await;

        // 提前转换字符串以避免临时值问题
        let video_path_str = video_path.to_string_lossy().to_string();
        let audio_path_str = audio_path.to_string_lossy().to_string();
        let output_path_str = output_path.to_string_lossy().to_string();

        let mut args = vec![
            "-i", &video_path_str,
            "-i", &audio_path_str,
            "-c:v", "copy",
        ];

        // macOS HEVC 兼容性修复：使用 hvc1 标签替代 hev1
        if needs_hevc_fix {
            eprintln!("检测到 HEVC 视频，添加 macOS 兼容性修复 (hvc1 标签)");
            args.extend(["-tag:v", "hvc1"]);
        }

        args.extend(["-c:a", "copy", "-y", &output_path_str]);

        // 打印命令（在消耗 args 之前）
        let cmd_str = format!("{} {}", self.ffmpeg_path, args.join(" "));
        eprintln!("FFmpeg 命令: {}", cmd_str);

        // 验证输入文件存在
        if !video_path.exists() {
            anyhow::bail!("视频文件不存在: {}", video_path.display());
        }
        if !audio_path.exists() {
            anyhow::bail!("音频文件不存在: {}", audio_path.display());
        }

        let output = tokio::process::Command::new(&self.ffmpeg_path)
            .args(&args)
            .output()
            .await?;

        eprintln!("FFmpeg 退出码: {:?}", output.status.code());

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("FFmpeg stderr: {}", stderr);
            anyhow::bail!("合并音视频失败: {}", stderr);
        }

        // 打印 FFmpeg 输出（可能有有用的信息）
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            eprintln!("FFmpeg stdout: {}", stdout);
        }

        Ok(())
    }

    /// 检测视频是否为 HEVC 编码
    async fn is_hevc_video(&self, video_path: &Path) -> bool {
        // 使用 ffmpeg 检测视频编码
        let probe_output = tokio::process::Command::new(&self.ffmpeg_path)
            .args(["-i", &video_path.to_string_lossy(), "-hide_banner"])
            .output()
            .await;

        match probe_output {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // 检查是否包含 hevc、h265 等关键字
                stderr.to_lowercase().contains("hevc") ||
                stderr.to_lowercase().contains("h265") ||
                stderr.to_lowercase().contains("hev1")
            }
            Err(_) => false,
        }
    }

}
