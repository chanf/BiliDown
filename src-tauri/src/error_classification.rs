use std::time::Duration;

/// 错误类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 网络连接错误（DNS、连接拒绝等）
    Connection,
    /// 超时错误
    Timeout,
    /// 服务器错误（4xx, 5xx）
    Server,
    /// 客户端错误（416 Range不可满足等）
    Client,
    /// 文件系统错误
    FileSystem,
    /// 未知错误
    Unknown,
}

/// 重试策略
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    /// 是否应该重试
    pub should_retry: bool,
    /// 重试前等待时间（秒）
    pub retry_delay: u64,
    /// 最大重试次数
    pub max_retries: usize,
    /// 是否使用指数退避
    pub use_exponential_backoff: bool,
}

/// 根据错误消息分类错误类型
pub fn classify_error(error_message: &str) -> ErrorCategory {
    let msg = error_message.to_lowercase();

    // 超时错误
    if msg.contains("timeout") || msg.contains("超时") {
        return ErrorCategory::Timeout;
    }

    // 连接错误
    if msg.contains("connection")
        || msg.contains("连接")
        || msg.contains("connect")
        || msg.contains("dns")
        || msg.contains("network")
        || msg.contains("网络") {
        return ErrorCategory::Connection;
    }

    // 服务器错误
    if msg.contains("403")
        || msg.contains("404")
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("status")
        || msg.contains("状态码") {
        return ErrorCategory::Server;
    }

    // 客户端错误（Range不可满足等）
    if msg.contains("416")
        || msg.contains("range")
        || msg.contains("范围") {
        return ErrorCategory::Client;
    }

    // 文件系统错误
    if msg.contains("文件")
        || msg.contains("file")
        || msg.contains("disk")
        || msg.contains("磁盘")
        || msg.contains("permission")
        || msg.contains("权限") {
        return ErrorCategory::FileSystem;
    }

    ErrorCategory::Unknown
}

/// 根据错误类型获取重试策略
pub fn get_retry_strategy(error_category: ErrorCategory, default_max_retry: usize) -> RetryStrategy {
    match error_category {
        ErrorCategory::Connection => {
            // 连接错误：快速重试，指数退避
            RetryStrategy {
                should_retry: true,
                retry_delay: 2,  // 2秒后重试
                max_retries: default_max_retry.max(3),
                use_exponential_backoff: true,
            }
        }
        ErrorCategory::Timeout => {
            // 超时错误：延长等待时间
            RetryStrategy {
                should_retry: true,
                retry_delay: 5,  // 5秒后重试
                max_retries: default_max_retry.max(3),
                use_exponential_backoff: false,  // 固定间隔
            }
        }
        ErrorCategory::Server => {
            // 服务器错误：指数退避，较长等待
            RetryStrategy {
                should_retry: true,
                retry_delay: 3,  // 3秒后重试
                max_retries: default_max_retry.max(2),  // 减少重试次数
                use_exponential_backoff: true,
            }
        }
        ErrorCategory::Client => {
            // 客户端错误（如416）：不重试
            RetryStrategy {
                should_retry: false,
                retry_delay: 0,
                max_retries: 0,
                use_exponential_backoff: false,
            }
        }
        ErrorCategory::FileSystem => {
            // 文件系统错误：不重试
            RetryStrategy {
                should_retry: false,
                retry_delay: 0,
                max_retries: 0,
                use_exponential_backoff: false,
            }
        }
        ErrorCategory::Unknown => {
            // 未知错误：保守重试
            RetryStrategy {
                should_retry: true,
                retry_delay: 3,
                max_retries: default_max_retry.max(2),
                use_exponential_backoff: true,
            }
        }
    }
}

/// 计算重试延迟时间
pub fn calculate_retry_delay(attempt: usize, strategy: &RetryStrategy) -> Duration {
    if strategy.use_exponential_backoff {
        // 指数退避：2^attempt 秒，最大 60 秒
        let delay = strategy.retry_delay * 2u64.pow(attempt as u32);
        let capped_delay = delay.min(60);
        Duration::from_secs(capped_delay)
    } else {
        // 固定间隔
        Duration::from_secs(strategy.retry_delay)
    }
}

/// 判断是否应该快速失败（不再重试）
pub fn should_fast_fail(error_category: ErrorCategory, attempt: usize) -> bool {
    match error_category {
        ErrorCategory::Client => true,  // 客户端错误立即失败
        ErrorCategory::FileSystem => true,  // 文件系统错误立即失败
        ErrorCategory::Server if attempt >= 2 => true,  // 服务器错误重试2次后快速失败
        _ => false,
    }
}
