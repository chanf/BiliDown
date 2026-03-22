use anyhow::Result;
use std::path::Path;
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
        let temp_dir = output_path.parent().unwrap_or_else(|| Path::new("."));
        
        let video_combined = temp_dir.join("video_combined.mp4");
        let audio_combined = temp_dir.join("audio_combined.m4a");

        self.merge_chunks(video_paths, &video_combined, "video").await?;
        self.merge_chunks(audio_paths, &audio_combined, "audio").await?;

        self.merge_streams(&video_combined, &audio_combined, output_path).await?;

        let _ = progress_tx.send(1.0).await;

        tokio::fs::remove_file(&video_combined).await.ok();
        tokio::fs::remove_file(&audio_combined).await.ok();

        Ok(())
    }

    async fn merge_chunks(
        &self,
        chunks: &[std::path::PathBuf],
        output_path: &Path,
        stream_type: &str,
    ) -> Result<()> {
        if chunks.len() == 1 {
            tokio::fs::copy(&chunks[0], output_path).await?;
            return Ok(());
        }

        let file_list_path = output_path.with_extension("txt");
        let mut file_list_content = String::new();
        for chunk in chunks {
            file_list_content.push_str(&format!("file '{}'\n", chunk.display()));
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

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("合并音视频失败: {}", stderr);
        }

        Ok(())
    }

    pub async fn merge_chunks_and_streams(
        &self,
        video_chunks: &[std::path::PathBuf],
        audio_chunks: &[std::path::PathBuf],
        output_path: &Path,
        progress_tx: Sender<f32>,
    ) -> Result<()> {
        self.merge(video_chunks, audio_chunks, output_path, progress_tx).await
    }
}
