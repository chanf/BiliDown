pub mod detector;

pub use detector::FFmpegDetector;

use std::path::PathBuf;

/// FFmpeg 管理器
pub struct FFmpegManager {
    pub path: PathBuf,
}

impl FFmpegManager {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &str {
        self.path.to_str().unwrap()
    }
}
