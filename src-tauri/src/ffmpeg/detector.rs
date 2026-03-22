use anyhow::Result;
use reqwest::Client;
use std::path::{Path, PathBuf};

pub struct FFmpegDetector {
    app_dir: PathBuf,
    client: Client,
}

impl FFmpegDetector {
    pub fn new(app_dir: PathBuf) -> Self {
        Self {
            app_dir,
            client: Client::new(),
        }
    }

    pub async fn detect_system_ffmpeg(&self) -> Option<String> {
        if let Ok(output) = tokio::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
        {
            if output.status.success() {
                return Some("ffmpeg".to_string());
            }
        }
        None
    }

    pub async fn get_or_install_ffmpeg(&self) -> Result<String> {
        if let Some(path) = self.detect_system_ffmpeg().await {
            return Ok(path);
        }

        let local_ffmpeg = self.app_dir.join("bin").join("ffmpeg");
        if local_ffmpeg.exists() {
            return Ok(local_ffmpeg.to_string_lossy().to_string());
        }

        self.download_and_install_ffmpeg().await?;

        Ok(local_ffmpeg.to_string_lossy().to_string())
    }

    async fn download_and_install_ffmpeg(&self) -> Result<()> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);

        let url = match (os, arch) {
            ("macos", "aarch64") => {
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-macos-arm64.tar.xz"
            }
            ("macos", "x86_64") => {
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-macos-x64.tar.xz"
            }
            ("linux", "x86_64") => {
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
            }
            ("windows", "x86_64") => {
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
            }
            _ => anyhow::bail!("不支持的系统: {} {}", os, arch),
        };

        let bin_dir = self.app_dir.join("bin");
        tokio::fs::create_dir_all(&bin_dir).await?;

        let response = self.client.get(url).send().await?;
        let bytes = response.bytes().await?;

        let is_zip = url.ends_with(".zip");
        let archive_path = bin_dir.join(if is_zip { "ffmpeg.zip" } else { "ffmpeg.tar.xz" });
        tokio::fs::write(&archive_path, &bytes).await?;

        if is_zip {
            self.extract_zip(&archive_path, &bin_dir).await?;
        } else {
            self.extract_tar(&archive_path, &bin_dir).await?;
        }

        tokio::fs::remove_file(archive_path).await.ok();

        Ok(())
    }

    #[cfg(unix)]
    async fn extract_tar(&self, archive_path: &Path, bin_dir: &Path) -> Result<()> {
        let output = tokio::process::Command::new("tar")
            .arg("xf")
            .arg(archive_path)
            .arg("-C")
            .arg(bin_dir)
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!("解压失败: {}", String::from_utf8_lossy(&output.stderr));
        }

        self.find_and_move_ffmpeg(bin_dir).await?;

        Ok(())
    }

    #[cfg(windows)]
    async fn extract_zip(&self, archive_path: &Path, bin_dir: &Path) -> Result<()> {
        let file = std::fs::File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = bin_dir.join(file.name());

            if file.name().ends_with('/') {
                tokio::fs::create_dir_all(&outpath).await?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        tokio::fs::create_dir_all(p).await?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }

        self.find_and_move_ffmpeg(bin_dir).await?;

        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    async fn extract_tar(&self, _archive_path: &Path, _bin_dir: &Path) -> Result<()> {
        anyhow::bail!("不支持的平台")
    }

    #[cfg(not(windows))]
    async fn extract_zip(&self, _archive_path: &Path, _bin_dir: &Path) -> Result<()> {
        anyhow::bail!("不支持的平台")
    }

    async fn find_and_move_ffmpeg(&self, bin_dir: &Path) -> Result<()> {
        let mut entries = tokio::fs::read_dir(bin_dir).await?;
        let ffmpeg_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        let ffmpeg_path = bin_dir.join(ffmpeg_name);

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                let potential_ffmpeg = path.join(ffmpeg_name);
                if potential_ffmpeg.exists() {
                    tokio::fs::rename(&potential_ffmpeg, &ffmpeg_path).await?;
                    break;
                }
            }
        }

        #[cfg(unix)]
        {
            tokio::process::Command::new("chmod")
                .arg("+x")
                .arg(&ffmpeg_path)
                .output()
                .await?;
        }

        Ok(())
    }

    pub async fn verify_ffmpeg(&self, path: &str) -> bool {
        if let Ok(output) = tokio::process::Command::new(path)
            .arg("-version")
            .output()
            .await
        {
            output.status.success()
        } else {
            false
        }
    }
}
