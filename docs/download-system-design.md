# B站视频下载系统 - 设计文档

## 一、架构概览

### 技术栈
- **后端**: Rust + Tauri 2.x + tokio (异步运行时)
- **前端**: React 19 + TypeScript
- **外部工具**: FFmpeg (音视频合并)

### 目录结构
```
src-tauri/src/
├── lib.rs              # 应用入口，管理全局状态
├── commands.rs         # Tauri 命令实现
├── bilibili.rs         # B站 API 客户端
├── login.rs            # 登录模块
├── downloader/         # 下载模块 (新增)
│   ├── mod.rs          # 模块导出
│   ├── manager.rs      # 任务管理器
│   ├── task.rs         # 单任务执行
│   ├── chunked.rs      # 分块下载
│   ├── resume.rs       # 断点续传
│   └── merger.rs       # FFmpeg 合并
└── ffmpeg/             # FFmpeg 模块 (新增)
    ├── mod.rs          # 模块导出
    └── detector.rs     # 检测和安装
```

## 二、核心数据结构

### 2.1 下载任务状态

```rust
/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,           // 等待中
    Downloading,       // 下载中
    Paused,            // 已暂停
    Merging,           // 合并中
    Completed,         // 已完成
    Failed(String),    // 失败(附带错误信息)
}

/// 下载任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub task_id: String,              // 唯一标识 (UUID)
    pub bvid: String,                 // 视频ID
    pub cid: i64,                     // 分P ID
    pub title: String,                // 视频标题
    pub part_title: Option<String>,   // 分P标题
    pub status: TaskStatus,           // 任务状态
    pub video_progress: f32,          // 视频下载进度 (0-1)
    pub audio_progress: f32,          // 音频下载进度 (0-1)
    pub video_size: u64,              // 视频总大小(字节)
    pub audio_size: u64,              // 音频总大小(字节)
    pub video_downloaded: u64,        // 视频已下载(字节)
    pub audio_downloaded: u64,        // 音频已下载(字节)
    pub speed: u64,                   // 当前速度(字节/秒)
    pub save_path: String,            // 保存路径
    pub filename: String,             // 最终文件名
    pub created_at: i64,              // 创建时间戳
    pub updated_at: i64,              // 更新时间戳
}
```

### 2.2 下载配置

```rust
/// 下载配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub save_path: String,            // 保存路径 (默认 ~/Movies)
    pub concurrent_connections: usize,// 并发连接数 (默认 4)
    pub chunk_size: usize,            // 分块大小 (默认 1MB)
    pub quality: i32,                 // 视频质量 (80=1080P, 64=720P)
    pub max_retry: usize,             // 最大重试次数 (默认 3)
    pub timeout: u64,                 // 超时时间 (秒，默认 30)
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            save_path: dirs::home_dir()
                .unwrap()
                .join("Movies")
                .to_string_lossy()
                .to_string(),
            concurrent_connections: 4,
            chunk_size: 1024 * 1024,  // 1MB
            quality: 80,               // 1080P
            max_retry: 3,
            timeout: 30,
        }
    }
}
```

### 2.3 全局状态

```rust
/// 全局下载状态
struct DownloadState {
    tasks: Mutex<HashMap<String, DownloadTask>>,
    active_tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    config: Mutex<DownloadConfig>,
    ffmpeg_path: Mutex<Option<String>>,
}
```

### 2.4 断点续传元数据

```rust
/// 断点续传元数据
#[derive(Debug, Serialize, Deserialize)]
pub struct ResumeMetadata {
    pub task_id: String,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
    pub video_url: String,
    pub audio_url: String,
    pub video_size: u64,
    pub audio_size: u64,
    pub video_chunks: Vec<ChunkInfo>,
    pub audio_chunks: Vec<ChunkInfo>,
    pub config: DownloadConfig,
    pub created_at: i64,
}

/// 分块信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub index: usize,       // 分块索引
    pub start: u64,         // 起始位置
    pub end: u64,           // 结束位置
    pub downloaded: bool,   // 是否已下载
    pub path: String,       // 临时文件路径
}
```

## 三、模块设计

### 3.1 downloader/manager.rs - 任务管理器

**职责**:
- 管理所有下载任务的生命周期
- 维护任务队列
- 处理暂停/恢复/删除操作
- 定期保存断点续传元数据

**关键接口**:
```rust
pub struct DownloadManager {
    state: Arc<DownloadState>,
    client: BilibiliClient,
    event_emitter: EventEmitter,
}

impl DownloadManager {
    pub fn new(state: Arc<DownloadState>, app: tauri::AppHandle) -> Self;

    /// 添加下载任务
    pub async fn add_task(&self, bvid: &str, cid: i64, title: &str, part_title: Option<String>) -> Result<String>;

    /// 暂停任务
    pub async fn pause_task(&self, task_id: &str) -> Result<()>;

    /// 恢复任务
    pub async fn resume_task(&self, task_id: &str) -> Result<()>;

    /// 删除任务 (包括临时文件)
    pub async fn delete_task(&self, task_id: &str, clean_files: bool) -> Result<()>;

    /// 获取所有任务
    pub async fn get_tasks(&self) -> Vec<DownloadTask>;

    /// 更新任务进度
    pub async fn update_task_progress(&self, task_id: &str, update: ProgressUpdate) -> Result<()>;
}
```

### 3.2 downloader/task.rs - 单任务执行

**职责**:
- 执行单个视频的下载流程
- 协调视频/音频下载
- 触发合并操作
- 推送进度事件

**关键接口**:
```rust
pub struct DownloadTaskRunner {
    task_id: String,
    metadata: ResumeMetadata,
    cancel_token: CancellationToken,
    pause_token: CancellationToken,
    manager: Arc<DownloadManager>,
    app: tauri::AppHandle,
}

impl DownloadTaskRunner {
    pub fn new(task_id: String, metadata: ResumeMetadata, manager: Arc<DownloadManager>, app: tauri::AppHandle) -> Self;

    /// 运行下载任务
    pub async fn run(&self) -> Result<()>;

    /// 暂停任务
    pub fn pause(&self);

    /// 检查是否应该暂停
    fn should_pause(&self) -> bool;
}
```

**执行流程**:
1. 检查暂停标记
2. 下载视频流 (分块并发)
3. 下载音频流 (分块并发)
4. 更新进度到前端
5. 调用 FFmpeg 合并
6. 清理临时文件
7. 更新任务状态为完成

### 3.3 downloader/chunked.rs - 分块下载

**职责**:
- 实现多线程分块下载
- 支持 HTTP Range 请求
- 处理下载重试和错误恢复

**关键接口**:
```rust
pub struct ChunkedDownloader {
    client: reqwest::Client,
    config: DownloadConfig,
}

impl ChunkedDownloader {
    pub fn new(config: DownloadConfig) -> Self;

    /// 分块下载文件
    pub async fn download_chunked(
        &self,
        url: &str,
        total_size: u64,
        output_dir: &Path,
        progress_tx: tokio::sync::mpsc::Sender<ProgressUpdate>,
        cancel_token: &CancellationToken,
    ) -> Result<Vec<PathBuf>>;  // 返回分块文件路径列表

    /// 下载单个分块
    async fn download_chunk(
        &self,
        url: &str,
        chunk: &ChunkInfo,
        cancel_token: &CancellationToken,
    ) -> Result<PathBuf>;
}

/// 进度更新
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub chunk_index: usize,
    pub downloaded: u64,
    pub total: u64,
}
```

### 3.4 downloader/resume.rs - 断点续传

**职责**:
- 保存和加载断点续传元数据
- 管理临时文件
- 检测可恢复的任务

**关键接口**:
```rust
pub struct ResumeManager {
    metadata_dir: PathBuf,
    temp_dir: PathBuf,
}

impl ResumeManager {
    pub fn new(base_dir: &Path) -> Self;

    /// 保存元数据
    pub async fn save_metadata(&self, metadata: &ResumeMetadata) -> Result<()>;

    /// 加载元数据
    pub async fn load_metadata(&self, task_id: &str) -> Option<ResumeMetadata>;

    /// 删除元数据
    pub async fn delete_metadata(&self, task_id: &str) -> Result<()>;

    /// 列出所有可恢复的任务
    pub async fn list_resumable_tasks(&self) -> Vec<ResumeMetadata>;

    /// 清理临时文件
    pub async fn cleanup_temp_files(&self, task_id: &str) -> Result<()>;

    /// 生成任务ID
    pub fn generate_task_id(&self, bvid: &str, cid: i64) -> String;
}
```

**元数据存储路径**: `~/.cache/bilibili-downloader/resume/{task_id}.json`

**临时文件路径**: `~/.cache/bilibili-downloader/temp/{task_id}/`

### 3.5 downloader/merger.rs - FFmpeg 合并

**职责**:
- 调用 FFmpeg 合并视频和音频
- 处理合并进度
- 错误处理和重试

**关键接口**:
```rust
pub struct VideoMerger {
    ffmpeg_path: String,
}

impl VideoMerger {
    pub fn new(ffmpeg_path: String) -> Self;

    /// 合并视频和音频
    pub async fn merge(
        &self,
        video_paths: &[PathBuf],
        audio_paths: &[PathBuf],
        output_path: &Path,
        progress_tx: tokio::sync::mpsc::Sender<f32>,
    ) -> Result<()>;

    /// 先合并分块，再合并音视频
    pub async fn merge_chunks_and_streams(
        &self,
        video_chunks: &[PathBuf],
        audio_chunks: &[PathBuf],
        output_path: &Path,
        progress_tx: tokio::sync::mpsc::Sender<f32>,
    ) -> Result<()>;
}
```

**FFmpeg 命令**:
```bash
# 1. 合并视频分块
ffmpeg -f concat -safe 0 -i file_list.txt -c copy video_combined.mp4

# 2. 合并音频分块
ffmpeg -f concat -safe 0 -i file_list.txt -c copy audio_combined.m4a

# 3. 合并音视频
ffmpeg -i video_combined.mp4 -i audio_combined.m4a -c:v copy -c:a copy output.mp4
```

### 3.6 ffmpeg/detector.rs - FFmpeg 检测和安装

**职责**:
- 检测系统 FFmpeg
- 下载并安装 FFmpeg 到项目目录
- 管理 FFmpeg 版本

**关键接口**:
```rust
pub struct FFmpegDetector {
    app_dir: PathBuf,
    client: reqwest::Client,
}

impl FFmpegDetector {
    pub fn new(app_dir: PathBuf) -> Self;

    /// 检测系统 FFmpeg
    pub async fn detect_system_ffmpeg(&self) -> Option<String>;

    /// 获取或安装 FFmpeg
    pub async fn get_or_install_ffmpeg(&self) -> Result<String>;

    /// 下载 FFmpeg
    async fn download_ffmpeg(&self) -> Result<PathBuf>;

    /// 解压 FFmpeg
    async fn extract_ffmpeg(&self, archive_path: &Path) -> Result<PathBuf>;

    /// 验证 FFmpeg
    pub async fn verify_ffmpeg(&self, path: &str) -> bool;
}
```

**下载源**:
- macOS arm64: `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-macos-arm64.tar.xz`
- macOS x64: `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-macos-x64.tar.xz`
- Linux x64: `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz`
- Windows x64: `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip`

**安装路径**: `~/.local/share/bilibili-downloader/bin/ffmpeg`

## 四、B站 API 扩展

### 4.1 获取播放URL

在 `bilibili.rs` 中添加:

```rust
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

        let video_url = dash["video"][0]["baseUrl"].as_str().unwrap().to_string();
        let audio_url = dash["audio"][0]["baseUrl"].as_str().unwrap().to_string();
        let video_size = dash["video"][0]["bandwidth"].as_i64().unwrap_or(0) as u64;
        let audio_size = dash["audio"][0]["bandwidth"].as_i64().unwrap_or(0) as u64;

        Ok(PlayUrlResult {
            video_url,
            audio_url,
            video_quality: data["quality"].as_i64().unwrap() as i32,
            video_size,
            audio_size,
        })
    }
}
```

## 五、Tauri 命令

### 5.1 命令列表

在 `commands.rs` 中实现:

```rust
/// 开始下载
#[tauri::command]
pub async fn download(
    videos: Vec<VideoToDownload>,
    save_path: Option<String>,
    state: State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<String, String>

/// 暂停下载
#[tauri::command]
pub async fn pause_download(
    task_id: String,
    state: State<'_, DownloadState>,
) -> Result<(), String>

/// 恢复下载
#[tauri::command]
pub async fn resume_download(
    task_id: String,
    state: State<'_, DownloadState>,
) -> Result<(), String>

/// 删除下载
#[tauri::command]
pub async fn delete_download(
    task_id: String,
    clean_files: bool,
    state: State<'_, DownloadState>,
) -> Result<(), String>

/// 获取下载进度
#[tauri::command]
pub fn get_download_progress(state: State<'_, DownloadState>) -> Vec<DownloadTask>

/// 设置下载配置
#[tauri::command]
pub async fn set_download_config(
    config: DownloadConfig,
    state: State<'_, DownloadState>,
) -> Result<(), String>

/// 获取下载配置
#[tauri::command]
pub fn get_download_config(state: State<'_, DownloadState>) -> DownloadConfig
```

### 5.2 数据结构

```rust
/// 前端传递的视频下载信息
#[derive(Debug, Deserialize)]
pub struct VideoToDownload {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub part_title: Option<String>,
}
```

## 六、前端集成

### 6.1 状态管理

```typescript
interface DownloadTask {
  task_id: string;
  bvid: string;
  cid: number;
  title: string;
  part_title?: string;
  status: 'Pending' | 'Downloading' | 'Paused' | 'Merging' | 'Completed' | 'Failed';
  video_progress: number;
  audio_progress: number;
  speed: number;
  save_path: string;
  filename: string;
  error?: string;
}

function App() {
  const [downloadTasks, setDownloadTasks] = useState<DownloadTask[]>([]);
  const [downloadConfig, setDownloadConfig] = useState<DownloadConfig>({
    save_path: '',
    concurrent_connections: 4,
    quality: 80,
  });
  // ...
}
```

### 6.2 事件监听

```typescript
useEffect(() => {
  const unlisten = listen<DownloadTask>('download-progress', (event) => {
    setDownloadTasks(prev => {
      const index = prev.findIndex(t => t.task_id === event.payload.task_id);
      if (index >= 0) {
        const updated = [...prev];
        updated[index] = event.payload;
        return updated;
      }
      return [...prev, event.payload];
    });
  });

  return () => {
    unlisten.then(fn => fn());
  };
}, []);
```

### 6.3 下载控制

```typescript
async function handleDownload() {
  const videos = Array.from(selectedVideos).map(bvid => {
    const video = result!.videos.find(v => v.bvid === bvid);
    return {
      bvid: video.bvid,
      cid: video.cid,  // 需要从视频信息获取
      title: result!.title,
      part_title: video.title,
    };
  });

  const result = await invoke('download', { videos });
  console.log(result);
}

async function pauseTask(taskId: string) {
  await invoke('pause_download', { taskId });
}

async function resumeTask(taskId: string) {
  await invoke('resume_download', { taskId });
}

async function deleteTask(taskId: string, cleanFiles: boolean) {
  await invoke('delete_download', { taskId, cleanFiles });
}
```

## 七、依赖添加

在 `Cargo.toml` 中添加:

```toml
[dependencies]
# 现有依赖...
tokio-util = { version = "0.7", features = ["sync"] }
futures = "0.3"
uuid = { version = "1", features = ["v4", "serde"] }
dirs = "5"
flate2 = "1.0"
tar = "0.4"
zip = "2.1"
```

## 八、实施顺序

### Phase 1: 基础架构 (优先)
1. 创建模块文件结构
2. 实现数据结构
3. 扩展 lib.rs 添加 DownloadState
4. 更新 Cargo.toml 依赖

### Phase 2: FFmpeg 集成
1. 实现 ffmpeg/detector.rs
2. 测试自动安装功能

### Phase 3: B站 API 扩展
1. 实现 get_play_url
2. 测试获取视频流

### Phase 4: 分块下载
1. 实现 downloader/chunked.rs
2. 测试 Range 下载

### Phase 5: 任务管理
1. 实现 downloader/manager.rs
2. 实现 downloader/task.rs
3. 测试任务创建和控制

### Phase 6: 断点续传
1. 实现 downloader/resume.rs
2. 测试暂停/恢复

### Phase 7: FFmpeg 合并
1. 实现 downloader/merger.rs
2. 测试视频合并

### Phase 8: 命令实现
1. 实现 commands.rs 所有命令
2. 测试命令调用

### Phase 9: 前端集成
1. 更新 App.tsx
2. 更新 App.css
3. 测试完整流程

## 九、关键常量

```rust
// 缓存目录
const CACHE_DIR: &str = ".cache/bilibili-downloader";
const RESUME_DIR: &str = "resume";
const TEMP_DIR: &str = "temp";

// FFmpeg 安装目录
const FFMPEG_DIR: &str = ".local/share/bilibili-downloader/bin";

// 进度更新间隔 (毫秒)
const PROGRESS_EMIT_INTERVAL_MS: u64 = 200;

// 默认分块大小 (1MB)
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

// 默认并发连接数
const DEFAULT_CONCURRENT: usize = 4;
```

## 十、错误处理

### 错误类型
```rust
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("网络错误: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("FFmpeg错误: {0}")]
    FFmpeg(String),

    #[error("任务已取消")]
    Cancelled,

    #[error("任务已暂停")]
    Paused,

    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),
}
```

### 重试策略
```rust
async fn retry_with_backoff<F, T, E>(
    mut f: F,
    max_retries: usize,
) -> Result<T, E>
where
    F: FnMut() -> Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
    E: std::fmt::Display,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_retries => {
                attempt += 1;
                let delay = Duration::from_secs(2u64.pow(attempt as u32));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

## 十一、测试检查点

- [ ] FFmpeg 自动安装成功
- [ ] 获取播放 URL 成功
- [ ] 分块下载正常工作
- [ ] 暂停后进度正确保存
- [ ] 恢复后从断点继续下载
- [ ] 删除后临时文件已清理
- [ ] 合并后视频可播放
- [ ] 进度实时更新到前端
- [ ] 多任务并发下载正常
- [ ] 默认保存路径为 ~/Movies
