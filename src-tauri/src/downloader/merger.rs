use anyhow::Result;
use std::path::{Path, PathBuf};
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

        let file_list_path = output_path.with_extension("txt");
        let mut file_list_content = String::new();
        for chunk in chunks {
            // 使用绝对路径并正确转义
            let absolute_path = if chunk.is_absolute() {
                chunk.clone()
            } else {
                std::path::PathBuf::from(&chunk.canonicalize().unwrap_or_else(|_| chunk.clone()))
            };
            // Windows 风格路径需要额外处理反斜杠
            let path_str = absolute_path.to_string_lossy()
                .replace('\\', "\\\\")
                .replace("'", "\\'");
            file_list_content.push_str(&format!("file '{}'\n", path_str));
        }
        tokio::fs::write(&file_list_path, file_list_content).await?;

        let codec = if stream_type == "video" { "copy" } else { "copy" };

        let output = tokio::process::Command::new(&self.ffmpeg_path)
            .args([
                "-f", "concat",
                "-safe", "0",
                "-i", &file_list_path.to_string_lossy(),
                "-c", codec,
                "-y",
                &output_path.to_string_lossy(),
            ])
            .output()
            .await?;

        tokio::fs::remove_file(&file_list_path).await.ok();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("合并{}分块失败: {}", stream_type, stderr);
        }

        Ok(())
    }

    async fn merge_streams(
        &self,
        video_path: &Path,
        audio_path: &Path,
        output_path: &Path,
    ) -> Result<()> {
        eprintln!("FFmpeg 命令: {} -i {} -i {} -c:v copy -c:a copy -y {}",
                 self.ffmpeg_path,
                 video_path.display(),
                 audio_path.display(),
                 output_path.display());

        // 验证输入文件存在
        if !video_path.exists() {
            anyhow::bail!("视频文件不存在: {}", video_path.display());
        }
        if !audio_path.exists() {
            anyhow::bail!("音频文件不存在: {}", audio_path.display());
        }

        let output = tokio::process::Command::new(&self.ffmpeg_path)
            .args([
                "-i", &video_path.to_string_lossy(),
                "-i", &audio_path.to_string_lossy(),
                "-c:v", "copy",
                "-c:a", "copy",
                "-y",
                &output_path.to_string_lossy(),
            ])
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

}
