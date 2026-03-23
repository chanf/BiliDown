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
        if self.verify_ffmpeg_path(&local_ffmpeg).await {
            return Ok(local_ffmpeg.to_string_lossy().to_string());
        }

        self.download_and_install_ffmpeg().await?;

        if !self.verify_ffmpeg_path(&local_ffmpeg).await {
            anyhow::bail!(
                "FFmpeg 安装后仍不可用: {}",
                local_ffmpeg.to_string_lossy()
            );
        }

        Ok(local_ffmpeg.to_string_lossy().to_string())
    }

    async fn download_and_install_ffmpeg(&self) -> Result<()> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);

        let url = match (os, arch) {
            // BtbN 当前已不再提供 macOS 资产，改用 evermeet 通用 zip
            ("macos", "aarch64") | ("macos", "x86_64") => {
                "https://evermeet.cx/ffmpeg/getrelease/zip"
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
        if !response.status().is_success() {
            anyhow::bail!("下载 FFmpeg 失败: {} ({})", url, response.status());
        }
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
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }

        self.find_and_move_ffmpeg(bin_dir).await?;

        Ok(())
    }

    #[cfg(not(windows))]
    async fn extract_zip(&self, archive_path: &Path, bin_dir: &Path) -> Result<()> {
        let file = std::fs::File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = bin_dir.join(file.name());

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }

        self.find_and_move_ffmpeg(bin_dir).await?;

        Ok(())
    }

    async fn find_and_move_ffmpeg(&self, bin_dir: &Path) -> Result<()> {
        let ffmpeg_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        let ffmpeg_path = bin_dir.join(ffmpeg_name);

        let found = self.find_ffmpeg_recursively(bin_dir, ffmpeg_name)?;
        let source_path = found.ok_or_else(|| anyhow::anyhow!("压缩包中未找到 ffmpeg 可执行文件"))?;

        if source_path != ffmpeg_path {
            match tokio::fs::rename(&source_path, &ffmpeg_path).await {
                Ok(_) => {}
                Err(_) => {
                    tokio::fs::copy(&source_path, &ffmpeg_path).await?;
                }
            }
        }

        #[cfg(unix)]
        {
            let output = tokio::process::Command::new("chmod")
                .arg("+x")
                .arg(&ffmpeg_path)
                .output()
                .await?;
            if !output.status.success() {
                anyhow::bail!(
                    "设置 ffmpeg 可执行权限失败: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        if !self.verify_ffmpeg_path(&ffmpeg_path).await {
            anyhow::bail!(
                "ffmpeg 可执行文件校验失败: {}",
                ffmpeg_path.to_string_lossy()
            );
        }

        Ok(())
    }

    fn find_ffmpeg_recursively(&self, root: &Path, ffmpeg_name: &str) -> Result<Option<PathBuf>> {
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == ffmpeg_name)
                    .unwrap_or(false)
                {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }

    async fn verify_ffmpeg_path(&self, path: &Path) -> bool {
        if !path.exists() {
            return false;
        }

        match tokio::process::Command::new(path)
            .arg("-version")
            .output()
            .await
        {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
}
