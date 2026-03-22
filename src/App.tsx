import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

interface PlaylistVideo {
  bvid: string;
  title: string;
  cid: number;
  index: number;
}

interface ParseResult {
  type: string;
  title: string;
  videos: PlaylistVideo[];
}

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

interface DownloadConfig {
  save_path: string;
  concurrent_connections: number;
  chunk_size: number;
  quality: number;
  max_retry: number;
  timeout: number;
}

function App() {
  const [url, setUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<ParseResult | null>(null);
  const [error, setError] = useState("");
  const [loggedIn, setLoggedIn] = useState(false);
  const [showQrcode, setShowQrcode] = useState(false);
  const [qrcodeImage, setQrcodeImage] = useState("");
  const [loginChecking, setLoginChecking] = useState(false);

  const [selectedVideos, setSelectedVideos] = useState<Set<number>>(new Set());
  const [downloadTasks, setDownloadTasks] = useState<DownloadTask[]>([]);
  const [showConfig, setShowConfig] = useState(false);
  const [config, setConfig] = useState<DownloadConfig>({
    save_path: '',
    concurrent_connections: 4,
    chunk_size: 1024 * 1024,
    quality: 80,
    max_retry: 3,
    timeout: 30,
  });

  async function parseUrl() {
    if (!url.trim()) {
      setError("请输入 B 站视频 URL");
      return;
    }

    setLoading(true);
    setError("");
    setSelectedVideos(new Set());
    try {
      const res = await invoke<ParseResult>("parse_url", { url });
      setResult(res);

      if (res.videos.length > 0) {
        setSelectedVideos(new Set(res.videos.map(v => v.cid)));
      }
    } catch (e) {
      setError(String(e));
      setResult(null);
    } finally {
      setLoading(false);
    }
  }

  async function checkLoginStatus() {
    try {
      const status = await invoke<{ logged_in: boolean }>("get_login_status");
      setLoggedIn(status.logged_in);
    } catch (e) {
      console.error("获取登录状态失败", e);
    }
  }

  async function handleLoginClick() {
    if (loggedIn) {
      setLoggedIn(false);
    } else {
      setShowQrcode(true);
      setLoginChecking(true);
      try {
        const qrcode = await invoke<{ url: string; qrcode_image: string }>("get_qrcode");
        setQrcodeImage(qrcode.qrcode_image);

        const pollInterval = setInterval(async () => {
          const status = await invoke<{ logged_in: boolean }>("get_login_status");
          if (status.logged_in) {
            setLoggedIn(true);
            setShowQrcode(false);
            setLoginChecking(false);
            clearInterval(pollInterval);
          }
        }, 2000);

        setTimeout(() => {
          clearInterval(pollInterval);
          setLoginChecking(false);
        }, 180000);
      } catch (e) {
        setError(String(e));
        setShowQrcode(false);
        setLoginChecking(false);
      }
    }
  }

  function handleSelectAll() {
    if (result && selectedVideos.size === result.videos.length) {
      setSelectedVideos(new Set());
    } else if (result) {
      setSelectedVideos(new Set(result.videos.map(v => v.cid)));
    }
  }

  function handleVideoSelect(cid: number) {
    const newSelected = new Set(selectedVideos);
    if (newSelected.has(cid)) {
      newSelected.delete(cid);
    } else {
      newSelected.add(cid);
    }
    setSelectedVideos(newSelected);
  }

  async function handleDownload() {
    if (selectedVideos.size === 0) {
      setError("请选择要下载的视频");
      return;
    }

    if (!result) {
      setError("请先解析视频链接");
      return;
    }

    const videos = result.videos
      .filter(video => selectedVideos.has(video.cid))
      .map(video => ({
        bvid: video.bvid,
        cid: video.cid,
        title: result.title,
        part_title: video.title,
      }));

    if (videos.length === 0) {
      setError("未找到选中的视频");
      return;
    }

    try {
      const downloadResult = await invoke<string>("download", { videos });
      console.log(downloadResult);
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }

  async function pauseTask(taskId: string) {
    try {
      await invoke("pause_download", { taskId });
    } catch (e) {
      setError(String(e));
    }
  }

  async function resumeTask(taskId: string) {
    try {
      await invoke("resume_download", { taskId });
    } catch (e) {
      setError(String(e));
    }
  }

  async function deleteTask(taskId: string) {
    try {
      await invoke("delete_download", { taskId, cleanFiles: true });
      setDownloadTasks(prev => prev.filter(t => t.task_id !== taskId));
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    checkLoginStatus();
    const interval = setInterval(checkLoginStatus, 5000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    loadConfig();
  }, []);

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

  async function loadConfig() {
    try {
      const loadedConfig = await invoke<DownloadConfig>("get_download_config");
      setConfig(loadedConfig);
    } catch (e) {
      console.error("加载配置失败", e);
    }
  }

  async function saveConfig() {
    try {
      await invoke("set_download_config", { config });
      setShowConfig(false);
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="container">
      <header>
        <div className="header-content">
          <div className="header-left">
            <h1>📺 B 站合集下载器</h1>
            <p className="subtitle">扫码登录 • 批量下载 • 离线观看</p>
          </div>
          <button
            className={`login-btn ${loggedIn ? 'logged-in' : ''}`}
            onClick={handleLoginClick}
          >
            {loggedIn ? '退出' : '扫码登录'}
          </button>
        </div>
      </header>

      <main>
        {showQrcode && (
          <div className="modal-overlay" onClick={() => setShowQrcode(false)}>
            <div className="modal-content" onClick={(e) => e.stopPropagation()}>
              <div className="modal-header">
                <h2>扫码登录 B 站</h2>
                <button className="close-btn" onClick={() => setShowQrcode(false)}>✕</button>
              </div>
              <div className="modal-body">
                {qrcodeImage && (
                  <div className="qrcode-container">
                    <img src={qrcodeImage} alt="B 站登录 QR 码" />
                    {loginChecking && <p className="checking">等待扫码...</p>}
                  </div>
                )}
                <p className="tip">请使用 B 站 App 扫描二维码登录</p>
              </div>
            </div>
          </div>
        )}

        <section className="url-input-section">
          <div className="input-group">
            <input
              id="url-input"
              type="text"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="粘贴 B 站视频 URL，例如: https://www.bilibili.com/video/BVxxx"
              onKeyDown={(e) => e.key === "Enter" && parseUrl()}
            />
            <button onClick={parseUrl} disabled={loading}>
              {loading ? "解析中..." : "解析"}
            </button>
          </div>
          {error && <p className="error">{error}</p>}
        </section>

        {result && (
          <section className="result-section">
            <div className="result-header">
              <div className="result-title">
                <p className="title">{result.title}</p>
              </div>
              <div className="selection-info">
                <span>已选 {selectedVideos.size} / {result.videos.length}</span>
              </div>
            </div>

            <div className="video-list">
              <div className="video-list-header">
                <label className="select-all">
                  <input
                    type="checkbox"
                    checked={result.videos.length > 0 && selectedVideos.size === result.videos.length}
                    onChange={handleSelectAll}
                  />
                  <span>全选</span>
                </label>
              </div>

              <div className="video-items">
                {result.videos.map((video) => (
                  <div key={video.cid} className="video-item">
                    <input
                      type="checkbox"
                      id={`video-${video.cid}`}
                      checked={selectedVideos.has(video.cid)}
                      onChange={() => handleVideoSelect(video.cid)}
                    />
                    <span className="index">{video.index}.</span>
                    <label htmlFor={`video-${video.cid}`} className="title">
                      {video.title}
                    </label>
                  </div>
                ))}
              </div>
            </div>

            <button
              className="btn-primary"
              onClick={handleDownload}
              disabled={selectedVideos.size === 0}
            >
              下载已选中的 {selectedVideos.size} 个视频
            </button>
          </section>
        )}

        <section className="download-section">
          <h2>📥 下载列表</h2>
          {downloadTasks.length === 0 ? (
            <p className="empty">暂无下载任务</p>
          ) : (
            <div className="download-list">
              {downloadTasks.map((task) => (
                <div key={task.task_id} className="download-item">
                  <div className="download-info">
                    <span className="download-title">{task.title}</span>
                    <span className={`download-status status-${task.status.toLowerCase()}`}>
                      {task.status === 'Pending' && '等待中'}
                      {task.status === 'Downloading' && '下载中'}
                      {task.status === 'Paused' && '已暂停'}
                      {task.status === 'Merging' && '合并中'}
                      {task.status === 'Completed' && '已完成'}
                      {task.status === 'Failed' && '失败'}
                    </span>
                  </div>
                  
                  {(task.status === 'Downloading' || task.status === 'Paused') && (
                    <div className="download-progress">
                      <div className="progress-bar">
                        <div 
                          className="progress-fill" 
                          style={{ width: `${(task.video_progress + task.audio_progress) / 2 * 100}%` }}
                        />
                      </div>
                      <span className="progress-text">
                        {Math.round((task.video_progress + task.audio_progress) / 2 * 100)}%
                      </span>
                    </div>
                  )}

                  <div className="download-actions">
                    {task.status === 'Downloading' && (
                      <button className="btn-action btn-pause" onClick={() => pauseTask(task.task_id)}>
                        暂停
                      </button>
                    )}
                    {task.status === 'Paused' && (
                      <button className="btn-action btn-resume" onClick={() => resumeTask(task.task_id)}>
                        恢复
                      </button>
                    )}
                    {(task.status === 'Pending' || task.status === 'Downloading' || task.status === 'Paused' || task.status === 'Failed') && (
                      <button className="btn-action btn-delete" onClick={() => deleteTask(task.task_id)}>
                        删除
                      </button>
                    )}
                  </div>

                  {task.status === 'Failed' && (
                    <p className="download-error">{task.error || '下载失败'}</p>
                  )}
                </div>
              ))}
            </div>
          )}
        </section>

        {showConfig && (
          <div className="modal-overlay" onClick={() => setShowConfig(false)}>
            <div className="modal-content config-modal" onClick={(e) => e.stopPropagation()}>
              <div className="modal-header">
                <h2>下载设置</h2>
                <button className="close-btn" onClick={() => setShowConfig(false)}>✕</button>
              </div>
              <div className="modal-body">
                <div className="config-item">
                  <label>保存路径</label>
                  <input
                    type="text"
                    value={config.save_path}
                    onChange={(e) => setConfig({ ...config, save_path: e.target.value })}
                    placeholder="~/Movies"
                  />
                </div>
                <div className="config-item">
                  <label>并发连接数</label>
                  <input
                    type="number"
                    min="1"
                    max="16"
                    value={config.concurrent_connections}
                    onChange={(e) => setConfig({ ...config, concurrent_connections: parseInt(e.target.value) })}
                  />
                </div>
                <div className="config-item">
                  <label>视频质量</label>
                  <select
                    value={config.quality}
                    onChange={(e) => setConfig({ ...config, quality: parseInt(e.target.value) })}
                  >
                    <option value={120}>4K</option>
                    <option value={116}>1080P 60fps</option>
                    <option value={112}>1080P+</option>
                    <option value={80}>1080P</option>
                    <option value={64}>720P</option>
                    <option value={32}>480P</option>
                  </select>
                </div>
                <div className="config-item">
                  <label>最大重试次数</label>
                  <input
                    type="number"
                    min="0"
                    max="10"
                    value={config.max_retry}
                    onChange={(e) => setConfig({ ...config, max_retry: parseInt(e.target.value) })}
                  />
                </div>
                <div className="config-actions">
                  <button className="btn-secondary" onClick={() => setShowConfig(false)}>取消</button>
                  <button className="btn-primary" onClick={saveConfig}>保存</button>
                </div>
              </div>
            </div>
          </div>
        )}
      </main>

      <footer>
        <div className="footer-content">
          <p>基于 Tauri + React 构建 | 使用前请先登录 B 站账号</p>
          <button className="config-btn" onClick={() => setShowConfig(true)}>⚙️ 设置</button>
        </div>
      </footer>
    </div>
  );
}

export default App;
