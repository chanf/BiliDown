import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
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

interface LogEntry {
  level: string;
  message: string;
  timestamp: string;
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
  const [pausingAll, setPausingAll] = useState(false);
  const [resumingAll, setResumingAll] = useState(false);
  const [retryingAll, setRetryingAll] = useState(false);
  const [showConfig, setShowConfig] = useState(false);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [showLogPanel, setShowLogPanel] = useState(false);
  const logContainerRef = useRef<HTMLDivElement>(null);
  const [toasts, setToasts] = useState<Array<{id: string; message: string; type: 'error' | 'warning' | 'success' | 'info'}>>([]);
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
        collection_type: result.type,
        collection_title: result.title,
      }));

    if (videos.length === 0) {
      setError("未找到选中的视频");
      return;
    }

    // 检查是否有重复的视频（只在下载中任务中检查，已完成的允许重复下载）
    const duplicateVideos: Array<{bvid: string; cid: number; part_title?: string; title: string}> = [];
    const newVideos = videos.filter(video => {
      // 只检查正在下载/暂停/待处理的任务，不包括已完成和失败的
      const exists = activeDownloadTasks.some(task =>
        task.bvid === video.bvid && task.cid === video.cid
      );
      if (exists) {
        duplicateVideos.push(video);
      }
      return !exists;
    });

    // 如果所有视频都重复（正在下载中），提示用户
    if (newVideos.length === 0) {
      setError(`所选视频正在下载中，请勿重复添加`);
      return;
    }

    // 如果部分视频重复，提示用户但继续下载新视频
    if (duplicateVideos.length > 0) {
      const duplicateTitles = duplicateVideos.map(v => v.part_title || v.title).join('、');
      setError(`跳过正在下载中的视频：${duplicateTitles}，继续下载 ${newVideos.length} 个新视频`);
    }

    try {
      const downloadResult = await invoke<string>("download", { videos: newVideos });
      console.log(downloadResult);
      // 如果部分重复，不清除错误提示，让用户看到跳过信息
      if (duplicateVideos.length === 0) {
        setError("");
      }
      setMainTab("download");
    } catch (e) {
      const errorMsg = String(e);
      // 长错误消息使用 toast 显示
      if (errorMsg.length > 50 || errorMsg.includes("所有质量等级都无法下载")) {
        addToast(errorMsg, 'error');
        setError("下载失败，请查看提示");
      } else {
        setError(errorMsg);
      }
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
        const errorMsg = `重试已创建新任务，但旧任务清理失败: ${String(deleteError)}`;
        addToast(errorMsg, 'warning');
      }
    } catch (e) {
      const errorMsg = String(e);
      // 长错误消息使用 toast 显示
      if (errorMsg.length > 50 || errorMsg.includes("所有质量等级都无法下载")) {
        addToast(`重试失败: ${errorMsg}`, 'error');
        setError("重试失败，请查看提示");
      } else {
        setError(`重试失败: ${errorMsg}`);
      }
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

  async function pauseAllTasks() {
    if (downloadingTasksCount === 0 || pausingAll || resumingAll) {
      return;
    }

    setPausingAll(true);
    try {
      const downloadingTasks = activeDownloadTasks.filter(
        (task) => getStatusText(task.status) === 'Downloading'
      );

      const taskIds = downloadingTasks.map((task) => task.task_id);
      const results = await Promise.allSettled(
        taskIds.map((taskId) => invoke("pause_download", { taskId }))
      );

      let successCount = 0;
      let failedCount = 0;

      results.forEach((result) => {
        if (result.status === "fulfilled") {
          successCount++;
        } else {
          failedCount++;
        }
      });

      if (failedCount > 0) {
        setError(`批量暂停完成：成功 ${successCount} 个，失败 ${failedCount} 个`);
      } else {
        setError("");
      }
    } catch (e) {
      setError(`批量暂停失败: ${String(e)}`);
    } finally {
      setPausingAll(false);
    }
  }

  async function resumeAllTasks() {
    if (pausedTasksCount === 0 || pausingAll || resumingAll) {
      return;
    }

    setResumingAll(true);
    try {
      const pausedTasks = activeDownloadTasks.filter(
        (task) => getStatusText(task.status) === 'Paused'
      );

      const taskIds = pausedTasks.map((task) => task.task_id);
      const results = await Promise.allSettled(
        taskIds.map((taskId) => invoke("resume_download", { taskId }))
      );

      let successCount = 0;
      let failedCount = 0;

      results.forEach((result) => {
        if (result.status === "fulfilled") {
          successCount++;
        } else {
          failedCount++;
        }
      });

      if (failedCount > 0) {
        setError(`批量恢复完成：成功 ${successCount} 个，失败 ${failedCount} 个`);
      } else {
        setError("");
      }
    } catch (e) {
      setError(`批量恢复失败: ${String(e)}`);
    } finally {
      setResumingAll(false);
    }
  }

  async function retryAllFailedTasks() {
    if (failedTasksCount === 0 || pausingAll || resumingAll || retryingAll) {
      return;
    }

    setRetryingAll(true);
    try {
      const failedTasks = activeDownloadTasks.filter(
        (task) => getStatusText(task.status) === 'Failed'
      );

      let successCount = 0;
      let failedCount = 0;
      const errors: string[] = [];

      // 逐个重试失败任务
      for (const task of failedTasks) {
        if (!task.bvid?.trim() || !Number.isFinite(task.cid) || task.cid <= 0 || !task.title?.trim()) {
          errors.push(`任务 "${task.title || task.part_title || '未知'}" 信息不完整`);
          failedCount++;
          continue;
        }

        try {
          // 标记为重试中
          setRetryingTaskIds((prev) => {
            const next = new Set(prev);
            next.add(task.task_id);
            return next;
          });

          // 重新创建下载任务
          await invoke("download", {
            videos: [{
              bvid: task.bvid,
              cid: task.cid,
              title: task.title,
              part_title: task.part_title,
            }],
            savePath: config.save_path || undefined,
          });

          // 删除旧任务（保留文件，因为新任务会覆盖）
          await invoke("delete_download", { taskId: task.task_id, cleanFiles: false });

          successCount++;
        } catch (e) {
          errors.push(`任务 "${task.title || task.part_title || '未知'}" 重试失败: ${String(e)}`);
          failedCount++;
        } finally {
          // 移除重试中标记
          setRetryingTaskIds((prev) => {
            const next = new Set(prev);
            next.delete(task.task_id);
            return next;
          });
        }
      }

      if (failedCount > 0) {
        const summary = `批量重试完成：成功 ${successCount} 个，失败 ${failedCount} 个`;
        setError(summary);
        // 详细错误使用 toast 显示
        if (errors.length > 0) {
          const errorDetails = errors.slice(0, 5).join('\n');
          const moreCount = errors.length > 5 ? `\n...还有 ${errors.length - 5} 个错误` : '';
          addToast(errorDetails + moreCount, 'warning');
        }
      } else if (successCount > 0) {
        setError(`已重新添加 ${successCount} 个下载任务`);
      } else {
        setError("");
      }
    } catch (e) {
      const errorMsg = `批量重试失败: ${String(e)}`;
      setError("批量重试失败，请查看提示");
      addToast(errorMsg, 'error');
    } finally {
      setRetryingAll(false);
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

  // 监听日志事件
  useEffect(() => {
    const unlisten = listen<LogEntry>('log-entry', (event) => {
      setLogs(prev => [...prev, event.payload]);
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, []);

  // 自动滚动到最新日志
  useEffect(() => {
    if (logContainerRef.current && showLogPanel) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs, showLogPanel]);

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

  async function copyLogs() {
    if (logs.length === 0) {
      setError("暂无日志可复制");
      return;
    }

    try {
      const logText = logs.map(log =>
        `[${log.timestamp}] [${log.level.toUpperCase()}] ${log.message}`
      ).join('\n');

      await writeText(logText);
      setError(`已复制 ${logs.length} 条日志`);
      setTimeout(() => setError(""), 2000);
    } catch (e) {
      setError(`复制日志失败: ${String(e)}`);
    }
  }

  function clearLogs() {
    setLogs([]);
    setError("已清空日志");
    setTimeout(() => setError(""), 2000);
  }

  function addToast(message: string, type: 'error' | 'warning' | 'success' | 'info' = 'info') {
    const id = Date.now().toString() + Math.random().toString();
    setToasts(prev => [...prev, { id, message, type }]);

    // 自动移除 toast（错误消息 8 秒，其他 3 秒）
    const duration = type === 'error' ? 8000 : 3000;
    setTimeout(() => {
      setToasts(prev => prev.filter(t => t.id !== id));
    }, duration);
  }

  async function openDownloadDir() {
    try {
      await invoke("open_download_dir");
    } catch (e) {
      setError(String(e));
    }
  }

  async function selectDownloadDir() {
    try {
      const selected = await invoke<string>("select_download_folder");
      if (selected) {
        setConfig({ ...config, save_path: selected });
      }
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

  // 统计正在下载和暂停中的任务数量
  const downloadingTasksCount = activeDownloadTasks.filter(
    (task) => getStatusText(task.status) === 'Downloading'
  ).length;

  const pausedTasksCount = activeDownloadTasks.filter(
    (task) => getStatusText(task.status) === 'Paused'
  ).length;

  const failedTasksCount = activeDownloadTasks.filter(
    (task) => getStatusText(task.status) === 'Failed'
  ).length;

  const parseVideoCount = result?.videos.length ?? 0;

  return (
    <div className="container">
      <header>
        <div className="header-content">
          <div className="header-left">
            <h1>📺 B 站合集下载器</h1>
            <p className="subtitle">扫码登录 • 批量下载 • 离线观看</p>
          </div>
          <div className="header-right">
            <button
              className="config-btn header-config-btn"
              onClick={() => setShowConfig(true)}
            >
              ⚙️ 设置
            </button>
            <button
              className={`login-btn ${loggedIn ? 'logged-in' : ''}`}
              onClick={handleLoginClick}
            >
              {loggedIn ? '退出' : '扫码登录'}
            </button>
          </div>
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
              <div className="download-controls">
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
                {downloadTab === "active" && (
                  <div className="batch-actions">
                    <button
                      type="button"
                      className="btn-batch btn-batch-pause"
                      onClick={pauseAllTasks}
                      disabled={pausingAll || resumingAll || downloadingTasksCount === 0}
                    >
                      {pausingAll ? "暂停中..." : "全部暂停"}
                    </button>
                    <button
                      type="button"
                      className="btn-batch btn-batch-resume"
                      onClick={resumeAllTasks}
                      disabled={pausingAll || resumingAll || pausedTasksCount === 0}
                    >
                      {resumingAll ? "恢复中..." : "全部下载"}
                    </button>
                    {failedTasksCount > 0 && (
                      <button
                        type="button"
                        className="btn-batch btn-batch-retry"
                        onClick={retryAllFailedTasks}
                        disabled={pausingAll || resumingAll || retryingAll}
                      >
                        {retryingAll ? `重试中... (${failedTasksCount})` : `重试失败 (${failedTasksCount})`}
                      </button>
                    )}
                  </div>
                )}
                {error && <p className="error">{error}</p>}
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
                  <label>下载目录</label>
                  <div className="path-input-group">
                    <input
                      type="text"
                      value={config.save_path}
                      onChange={(e) => setConfig({ ...config, save_path: e.target.value })}
                      placeholder="选择下载目录"
                    />
                    <button
                      type="button"
                      className="btn-browse"
                      onClick={selectDownloadDir}
                    >
                      浏览
                    </button>
                  </div>
                </div>
                <div className="config-item">
                  <label>
                    同时下载数量
                    <span className="config-hint">单个任务的并发连接数</span>
                  </label>
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

      {/* 日志窗口切换按钮 */}
      <button
        className="log-toggle-btn"
        onClick={() => setShowLogPanel(!showLogPanel)}
        title={showLogPanel ? "隐藏日志" : "显示日志"}
      >
        {showLogPanel ? '📖 隐藏日志' : '📋 显示日志'}
        {logs.length > 0 && ` (${logs.length})`}
      </button>

      {/* 日志窗口 */}
      {showLogPanel && (
        <div className="log-panel">
          <div className="log-panel-header">
            <h3>运行日志</h3>
            <div className="log-panel-actions">
              <button onClick={copyLogs} title="复制所有日志到剪切板">复制</button>
              <button onClick={clearLogs} title="清空所有日志">清空</button>
              <button onClick={() => setShowLogPanel(false)} title="关闭日志窗口">关闭</button>
            </div>
          </div>
          <div ref={logContainerRef} className="log-panel-content">
            {logs.length === 0 ? (
              <p className="log-empty">暂无日志</p>
            ) : (
              logs.map((log, index) => {
                // 从 level 字符串中提取实际级别
                const levelMatch = log.level.match(/"(\w+)"/);
                const level = levelMatch ? levelMatch[1] : 'info';

                return (
                  <div key={index} className={`log-entry log-${level}`}>
                    <span className="log-timestamp">[{log.timestamp}]</span>
                    <span className="log-level">[{level.toUpperCase()}]</span>
                    <span className="log-message">{log.message}</span>
                  </div>
                );
              })
            )}
          </div>
        </div>
      )}

      {/* Toast 提示组件 */}
      <div className="toast-container">
        {toasts.map(toast => (
          <div key={toast.id} className={`toast toast-${toast.type}`}>
            <div className="toast-content">
              {toast.type === 'error' && '⚠️ '}
              {toast.type === 'warning' && '⚡ '}
              {toast.type === 'success' && '✅ '}
              {toast.type === 'info' && 'ℹ️ '}
              {toast.message}
            </div>
            <button
              className="toast-close"
              onClick={() => setToasts(prev => prev.filter(t => t.id !== toast.id))}
            >
              ✕
            </button>
          </div>
        ))}
      </div>

      <footer>
        <div className="footer-content">
          <div className="footer-buttons">
            <button className="config-btn" onClick={openDownloadDir}>📁 打开目录</button>
          </div>
        </div>
      </footer>
    </div>
  );
}

export default App;
