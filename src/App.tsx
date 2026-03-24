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
  status: Record<string, unknown> | string;
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

// Helper function to extract status string from Rust enum serialization
function getStatusText(status: Record<string, unknown> | string): string {
  if (typeof status === 'string') {
    return status;
  }
  // Handle Rust enum serialization like {"Pending": null} or {"Failed": "message"}
  const keys = Object.keys(status);
  if (keys.length === 1) {
    return keys[0];
  }
  return 'Pending';
}

function getErrorFromStatus(status: Record<string, unknown> | string): string | undefined {
  if (typeof status === 'object' && status !== null) {
    if ('Failed' in status && typeof status.Failed === 'string') {
      return status.Failed;
    }
  }
  return undefined;
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
  const [mainTab, setMainTab] = useState<"parse" | "download">("parse");
  const [downloadTab, setDownloadTab] = useState<"active" | "completed">("active");
  const [clearingCompleted, setClearingCompleted] = useState(false);
  const [retryingTaskIds, setRetryingTaskIds] = useState<Set<string>>(new Set());
  const [showConfig, setShowConfig] = useState(false);
  const [config, setConfig] = useState<DownloadConfig>({
    save_path: '',
    concurrent_connections: 4,
    chunk_size: 1024 * 1024,
    quality: 80,
    max_retry: 3,
    timeout: 30,
  });

  async function parseUrl(inputUrl?: string) {
    const targetUrl = (inputUrl ?? url).trim();
    if (!targetUrl) {
      setError("请输入 B 站视频 URL");
      return;
    }

    setLoading(true);
    setError("");
    setSelectedVideos(new Set());
    try {
      const res = await invoke<ParseResult>("parse_url", { url: targetUrl });
      setResult(res);
      setMainTab("parse");

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

  async function handlePasteAndParse() {
    try {
      const text = (await invoke<string>("read_clipboard_text")).trim();
      if (!text) {
        setError("剪切板为空，请先复制 B 站视频 URL");
        return;
      }
      setUrl(text);
      await parseUrl(text);
    } catch (e) {
      setError(`读取剪切板失败: ${String(e)}`);
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
      // 退出登录
      try {
        await invoke("logout");
        setLoggedIn(false);
      } catch (e) {
        setError(String(e));
      }
    } else {
      // 显示二维码登录
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
      setMainTab("download");
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

  async function retryTask(task: DownloadTask) {
    if (retryingTaskIds.has(task.task_id)) {
      return;
    }

    if (!task.bvid?.trim() || !Number.isFinite(task.cid) || task.cid <= 0 || !task.title?.trim()) {
      setError("任务信息不完整，无法重试");
      return;
    }

    setRetryingTaskIds((prev) => {
      const next = new Set(prev);
      next.add(task.task_id);
      return next;
    });

    try {
      const video = {
        bvid: task.bvid,
        cid: task.cid,
        title: task.title,
        part_title: task.part_title,
      };
      await invoke<string>("download", { videos: [video] });
      setError("");
      setMainTab("download");

      try {
        await invoke("delete_download", { taskId: task.task_id, cleanFiles: true });
        setDownloadTasks((prev) => prev.filter((t) => t.task_id !== task.task_id));
      } catch (deleteError) {
        setError(`重试已创建新任务，但旧任务清理失败: ${String(deleteError)}`);
      }
    } catch (e) {
      setError(`重试失败: ${String(e)}`);
    } finally {
      setRetryingTaskIds((prev) => {
        const next = new Set(prev);
        next.delete(task.task_id);
        return next;
      });
    }
  }

  async function clearCompletedTasks() {
    if (completedDownloadTasks.length === 0 || clearingCompleted) {
      return;
    }

    setClearingCompleted(true);
    try {
      const completedIds = completedDownloadTasks.map((task) => task.task_id);
      const results = await Promise.allSettled(
        completedIds.map((taskId) =>
          invoke("delete_download", { taskId, cleanFiles: false })
        )
      );

      const successIds = new Set<string>();
      let failedCount = 0;

      results.forEach((result, index) => {
        if (result.status === "fulfilled") {
          successIds.add(completedIds[index]);
        } else {
          failedCount += 1;
        }
      });

      if (successIds.size > 0) {
        setDownloadTasks((prev) =>
          prev.filter((task) => !successIds.has(task.task_id))
        );
      }

      if (failedCount > 0) {
        setError(`清空列表时有 ${failedCount} 个任务失败，请重试`);
      } else {
        setError("");
      }
    } finally {
      setClearingCompleted(false);
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

  async function openDownloadDir() {
    try {
      await invoke("open_download_dir");
    } catch (e) {
      setError(String(e));
    }
  }

  const activeDownloadTasks = downloadTasks.filter(
    (task) => getStatusText(task.status) !== "Completed"
  );
  const completedDownloadTasks = downloadTasks.filter(
    (task) => getStatusText(task.status) === "Completed"
  );
  const displayDownloadTasks =
    downloadTab === "active" ? activeDownloadTasks : completedDownloadTasks;
  const parseVideoCount = result?.videos.length ?? 0;

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
            <button onClick={() => parseUrl()} disabled={loading}>
              {loading ? "解析中..." : "解析"}
            </button>
            <button
              className="btn-paste-parse"
              onClick={handlePasteAndParse}
              disabled={loading}
            >
              粘贴解析
            </button>
          </div>
          {error && <p className="error">{error}</p>}
        </section>

        <section className="main-tabs">
          <button
            type="button"
            className={`main-tab-btn ${mainTab === "parse" ? "active" : ""}`}
            onClick={() => setMainTab("parse")}
          >
            解析列表
            <span className="main-tab-count">{parseVideoCount}</span>
          </button>
          <button
            type="button"
            className={`main-tab-btn ${mainTab === "download" ? "active" : ""}`}
            onClick={() => setMainTab("download")}
          >
            下载列表
            <span className="main-tab-count">{downloadTasks.length}</span>
          </button>
        </section>

        {mainTab === "parse" && (
          result ? (
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
          ) : (
            <section className="result-section">
              <p className="empty">暂无解析列表，请先输入 URL 并点击解析</p>
            </section>
          )
        )}

        {mainTab === "download" && (
          <section className="download-section">
            <div className="download-header">
              <h2>📥 下载列表</h2>
              <div className="download-tabs">
                <button
                  type="button"
                  className={`download-tab-btn ${downloadTab === "active" ? "active" : ""}`}
                  onClick={() => setDownloadTab("active")}
                >
                  下载中
                  <span className="download-tab-count">{activeDownloadTasks.length}</span>
                </button>
                <button
                  type="button"
                  className={`download-tab-btn ${downloadTab === "completed" ? "active" : ""}`}
                  onClick={() => setDownloadTab("completed")}
                >
                  已下载
                  <span className="download-tab-count">{completedDownloadTasks.length}</span>
                </button>
              </div>
            </div>
            {displayDownloadTasks.length === 0 ? (
              <p className="empty">
                {downloadTab === "active" ? "暂无下载中任务" : "暂无已下载任务"}
              </p>
            ) : (
              <div className="download-list">
                {displayDownloadTasks.map((task) => {
                  const statusText = getStatusText(task.status);
                  const errorText = getErrorFromStatus(task.status) || task.error;
                  const displayTitle = (task.part_title && task.part_title.trim())
                    ? task.part_title
                    : task.title;
                  const isRetrying = retryingTaskIds.has(task.task_id);
                  return (
                    <div key={task.task_id} className="download-item">
                      <div className="download-info">
                        <span className="download-title">{displayTitle}</span>
                        <span className={`download-status status-${statusText.toLowerCase()}`}>
                          {statusText === 'Pending' && '等待中'}
                          {statusText === 'Downloading' && '下载中'}
                          {statusText === 'Paused' && '已暂停'}
                          {statusText === 'Merging' && '合并中'}
                          {statusText === 'Completed' && '已完成'}
                          {statusText === 'Failed' && '失败'}
                        </span>
                      </div>

                      {(statusText === 'Downloading' || statusText === 'Paused') && (
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
                        {statusText === 'Downloading' && (
                          <button className="btn-action btn-pause" onClick={() => pauseTask(task.task_id)}>
                            暂停
                          </button>
                        )}
                        {statusText === 'Paused' && (
                          <button className="btn-action btn-resume" onClick={() => resumeTask(task.task_id)}>
                            恢复
                          </button>
                        )}
                        {statusText === 'Failed' && (
                          <button
                            className="btn-action btn-retry"
                            onClick={() => retryTask(task)}
                            disabled={isRetrying}
                          >
                            {isRetrying ? '重试中...' : '重试'}
                          </button>
                        )}
                        {(statusText === 'Pending' || statusText === 'Downloading' || statusText === 'Paused' || statusText === 'Failed') && (
                          <button
                            className="btn-action btn-delete"
                            onClick={() => deleteTask(task.task_id)}
                            disabled={statusText === 'Failed' && isRetrying}
                          >
                            删除
                          </button>
                        )}
                      </div>

                      {statusText === 'Failed' && (
                        <p className="download-error">{errorText || '下载失败'}</p>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
            {downloadTab === "completed" && displayDownloadTasks.length > 0 && (
              <div className="download-footer-actions">
                <button
                  type="button"
                  className="btn-clear-completed"
                  onClick={clearCompletedTasks}
                  disabled={clearingCompleted}
                >
                  {clearingCompleted ? "清空中..." : "清空列表"}
                </button>
              </div>
            )}
          </section>
        )}

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
          <div className="footer-buttons">
            <button className="config-btn" onClick={openDownloadDir}>📁 打开目录</button>
            <button className="config-btn" onClick={() => setShowConfig(true)}>⚙️ 设置</button>
          </div>
        </div>
      </footer>
    </div>
  );
}

export default App;
