//! 简单指数退避重试策略（建连/首响应阶段）。

use std::time::Duration;

/// 可重试的 HTTP 状态（对齐常见网关语义）。
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504)
}

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// 首次请求之外的最大尝试次数。
    pub max_attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    /// 计算第 `attempt`（从 1 起，即第一次重试）失败后的等待时间。
    pub fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            // Retry-After 优先，但封顶到 max_delay 的 4 倍，避免服务端给天文数字
            return ra.min(self.max_delay * 4);
        }
        let exp = 2u32.saturating_pow(attempt.saturating_sub(1));
        let d = self.base.saturating_mul(exp).min(self.max_delay);
        // ±25% 抖动（确定性来源不追求，tokio 运行时使用简单纳秒）
        let jitter_us = (d.as_micros() as u64) / 4;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|x| x.subsec_nanos() as u64 % (jitter_us.max(1) * 2 + 1))
            .unwrap_or(0);
        let offset = now.saturating_sub(jitter_us);
        d.saturating_sub(Duration::from_micros(offset))
    }

    pub fn can_retry(&self, attempts_done: u32) -> bool {
        attempts_done < self.max_attempts
    }
}

/// 解析 `Retry-After` 头：秒数或 HTTP-date（M1 支持秒数）。
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let v = value?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_classification() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy::default();
        assert!(p.backoff(1, None) <= Duration::from_millis(625));
        assert!(p.backoff(10, None) <= Duration::from_secs(8));
        assert_eq!(
            p.backoff(1, Some(Duration::from_secs(30))),
            Duration::from_secs(30)
        );
        assert_eq!(
            p.backoff(1, Some(Duration::from_secs(999))),
            Duration::from_secs(32)
        );
    }

    #[test]
    fn retry_after_header_seconds() {
        assert_eq!(
            parse_retry_after(Some("120")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(parse_retry_after(Some("")), None);
        assert_eq!(parse_retry_after(None), None);
    }
}
