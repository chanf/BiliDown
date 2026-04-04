use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum LogLevel {
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
}

#[derive(Clone, serde::Serialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    pub timestamp: String,
}

// 全局 AppHandle
static APP_HANDLE: Mutex<Option<AppHandle>> = Mutex::new(None);

// 初始化日志系统
pub fn init(app_handle: AppHandle) {
    *APP_HANDLE.lock().unwrap() = Some(app_handle);
}

// 发送日志到前端
pub fn log(level: LogLevel, message: String) {
    if let Some(handle) = APP_HANDLE.lock().unwrap().as_ref() {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
        let _ = handle.emit("log-entry", LogEntry {
            level: serde_json::to_string(&level).unwrap_or_default(),
            message,
            timestamp,
        });
    }
}

// 便捷函数
pub fn log_info(msg: &str) {
    log(LogLevel::Info, msg.to_string());
}

pub fn log_warn(msg: &str) {
    log(LogLevel::Warning, msg.to_string());
}

pub fn log_error(msg: &str) {
    log(LogLevel::Error, msg.to_string());
}

pub fn log_debug(msg: &str) {
    log(LogLevel::Debug, msg.to_string());
}
