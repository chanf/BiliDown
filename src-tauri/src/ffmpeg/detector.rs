use anyhow::Result;
use reqwest::Client;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

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

    pub async fn detect_bundled_ffmpeg(&self) -> Option<String> {
        debug!("开始检测内置 FFmpeg");

        // 开发环境：尝试多个可能的路径
        let dev_paths = vec![
            std::path::PathBuf::from("src-tauri/resources/bin/ffmpeg"),
            std::path::PathBuf::from("resources/bin/ffmpeg"),
            std::path::PathBuf::from("../resources/bin/ffmpeg"),
        ];

        for path in &dev_paths {
            debug!("检查开发环境路径: {}", path.display());
            if self.verify_ffmpeg_path(path).await {
                info!("找到开发环境 FFmpeg: {}", path.display());
                return Some(path.to_string_lossy().to_string());
            }
        }

        // 生产环境：使用应用资源目录
        if let Ok(exe_path) = std::env::current_exe() {
            debug!("可执行文件路径: {}", exe_path.display());

            // macOS app bundle 结构: BiliDown.app/Contents/MacOS/bilibili-downloader
            // 资源在: BiliDown.app/Contents/Resources/bin/ffmpeg
            // 需要从 exe 向上两级到 Contents，然后进入 Resources
            if let Some(contents_dir) = exe_path.parent().and_then(|p| p.parent()) {
                let resources_dir = contents_dir.join("Resources"); // 注意 macOS 是大写 R
                debug!("Resources 目录: {}", resources_dir.display());

                // 尝试多个可能的资源路径
                let possible_paths = vec![
                    resources_dir.join("bin").join("ffmpeg"),         // Resources/bin/ffmpeg
                    resources_dir.join("resources").join("bin").join("ffmpeg"), // Resources/resources/bin/ffmpeg (Tauri 打包结构)
                ];

                for path in &possible_paths {
                    debug!("检查生产环境路径: {}", path.display());
                    if self.verify_ffmpeg_path(path).await {
                        info!("找到生产环境 FFmpeg: {}", path.display());
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }

            // Linux/Windows fallback: 资源在 exe 同级或父级的 resources 目录
            if let Some(exe_dir) = exe_path.parent() {
                let bundled_path = exe_dir.join("resources").join("bin").join("ffmpeg");
                debug!("检查 fallback 路径: {}", bundled_path.display());
                if self.verify_ffmpeg_path(&bundled_path).await {
                    info!("找到 fallback FFmpeg: {}", bundled_path.display());
                    return Some(bundled_path.to_string_lossy().to_string());
                }
            }
        }

        warn!("未找到内置 FFmpeg");
        None
    }

    pub async fn get_or_install_ffmpeg(&self) -> Result<String> {
        info!("开始获取或安装 FFmpeg");

        // 优先使用内置 FFmpeg
        if let Some(bundled_path) = self.detect_bundled_ffmpeg().await {
            info!("使用内置 FFmpeg: {}", bundled_path);
            return Ok(bundled_path);
        }

        // 次选系统 FFmpeg
        if let Some(path) = self.detect_system_ffmpeg().await {
            info!("使用系统 FFmpeg: {}", path);
            return Ok(path);
        }

        let local_ffmpeg = self.app_dir.join("bin").join("ffmpeg");
        if self.verify_ffmpeg_path(&local_ffmpeg).await {
            info!("使用本地缓存 FFmpeg: {}", local_ffmpeg.display());
            return Ok(local_ffmpeg.to_string_lossy().to_string());
        }

        info!("未找到可用 FFmpeg，开始下载安装");
        self.download_and_install_ffmpeg().await?;

        if !self.verify_ffmpeg_path(&local_ffmpeg).await {
            anyhow::bail!(
                "FFmpeg 安装后仍不可用: {}",
                local_ffmpeg.to_string_lossy()
            );
        }

        info!("FFmpeg 下载安装成功: {}", local_ffmpeg.display());
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
        // 检查文件是否存在
        if !path.exists() {
            debug!("FFmpeg 路径不存在: {}", path.display());
            return false;
        }

        // 尝试运行 ffmpeg -version，带重试机制
        for attempt in 1..=3 {
            match tokio::process::Command::new(path)
                .arg("-version")
                .output()
                .await
            {
                Ok(output) => {
                    if output.status.success() {
                        debug!("FFmpeg 验证成功: {}", path.display());
                        return true;
                    } else {
                        debug!(
                            "FFmpeg 验证失败 (尝试 {}/3): {} - 退出码: {:?}",
                            attempt,
                            path.display(),
                            output.status.code()
                        );
                    }
                }
                Err(e) => {
                    debug!(
                        "FFmpeg 验证错误 (尝试 {}/3): {} - {}",
                        attempt,
                        path.display(),
                        e
                    );
                }
            }

            // 如果不是最后一次尝试，等待一小段时间后重试
            if attempt < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }

        warn!("FFmpeg 验证失败（已重试 3 次）: {}", path.display());
        false
    }
}
