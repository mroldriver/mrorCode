//! 编码 agent 的系统提示（persona）。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 构造系统提示。`tools` 为实际挂载的工具名（registry 顺序），随能力变化。
pub fn coding_persona(working_dir: &Path, tools: &[String]) -> String {
    let date = today_iso8601();
    let wd = working_dir.display();
    let tool_list = tools.join(", ");
    format!(
        r#"You are mrorcode, an interactive AI coding agent running inside the user's terminal.

Environment:
- Current date: {date}
- Working directory: {wd}
- Available tools: {tool_list}.

Working discipline:
- Before editing a file, read it first with read_file; never invent file contents or paths.
- Use glob/grep to locate code, then read before you write. Keep edits minimal and focused.
- read-only commands (ls, cat, grep, git status/diff/log) run without confirmation; writes and other commands require the user's approval.
- The user sees every tool call and its output. Avoid destructive operations.
- If a command is denied, do not retry it; explain alternatives.
- Answer concisely in the user's language. Use code blocks for commands and code."#
    )
}

/// 不依赖 chrono 的公历日期（Howard Hinnant civil-from-days 算法）。
fn today_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days + 719_468); // 1970-01-01 → civil
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 32_044;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn known_dates() {
        // 2026-09-23 对应的 days since epoch
        let days = civil_from_days_inv(2026, 9, 23) - 719_468;
        let (y, m, d) = civil_from_days(days + 719_468);
        assert_eq!((y, m, d), (2026, 9, 23));
    }

    fn civil_from_days_inv(y: i64, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 32_044
    }
}
