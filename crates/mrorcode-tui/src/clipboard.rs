//! 剪贴板复制（M4-6）。
//!
//! 优先级：arboard（feature `clipboard`）→ OSC 52 转义序列 → 平台原生命令兜底。
//! 原生命令兜底：Windows 用 powershell Set-Clipboard，macOS 用 pbcopy，
//! Linux 先试 wl-copy（Wayland）再试 xclip（X11）。全部通过 stdin 管道传递文本，
//! 避免命令行注入与长度限制。

#![allow(dead_code)] // M4 步骤 6 接线前先建立实现与单测

/// 把文本写入剪贴板；返回是否成功（失败仅记录日志，不打断交互）。
pub fn copy(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    #[cfg(feature = "clipboard")]
    {
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.set_text(text).is_ok() {
                return true;
            }
        }
    }
    // OSC 52 优先（纯协议、SSH 友好）
    if copy_osc52(text) {
        return true;
    }
    // 兜底：平台原生命令
    copy_native(text)
}

/// OSC 52 复制：`ESC ] 52 ; c ; <base64> BEL`。
fn copy_osc52(text: &str) -> bool {
    use std::io::Write;
    let encoded = base64(text.as_bytes());
    let seq = format!("\u{1b}]52;c;{encoded}\u{07}");
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
}

/// 平台原生剪贴板兜底：通过 stdin 管道把文本写入系统剪贴板。
fn copy_native(text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        copy_via_pipe(
            &[
                "powershell",
                "-NoProfile",
                "-Command",
                "$input | Set-Clipboard",
            ],
            text,
        )
    }
    #[cfg(target_os = "macos")]
    {
        copy_via_pipe(&["pbcopy"], text)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Linux：先 Wayland (wl-copy)，再 X11 (xclip)
        copy_via_pipe(&["wl-copy"], text)
            || copy_via_pipe(&["xclip", "-selection", "clipboard"], text)
    }
    #[cfg(not(any(windows, target_os = "macos", unix)))]
    {
        let _ = text;
        false
    }
}

/// 启动命令并通过 stdin 管道写入文本，等待命令结束。成功返回 true。
fn copy_via_pipe(cmd_and_args: &[&str], text: &str) -> bool {
    if cmd_and_args.is_empty() {
        return false;
    }
    let (cmd, args) = cmd_and_args.split_first().unwrap();
    let mut child = match std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    // 写入 stdin
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    // 等待命令完成，检查退出码
    child.wait().map(|status| status.success()).unwrap_or(false)
}

/// 最小 base64 编码（zero-dependency）。
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64, copy_via_pipe};

    #[test]
    fn base64_encodes_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn copy_empty_text_returns_false() {
        assert!(!super::copy(""));
    }

    #[test]
    fn copy_via_pipe_returns_false_for_nonexistent_command() {
        assert!(!copy_via_pipe(&["nonexistent-command-xyz"], "hello"));
    }

    #[test]
    fn copy_via_pipe_returns_false_for_empty_cmd() {
        assert!(!copy_via_pipe(&[], "hello"));
    }

    #[test]
    fn copy_via_pipe_writes_to_stdin_successfully() {
        // 用一个一定存在的命令验证管道写入：Windows 用 findstr，Unix 用 cat
        #[cfg(target_os = "windows")]
        let cmd: &[&str] = &["findstr", "."];
        #[cfg(not(target_os = "windows"))]
        let cmd: &[&str] = &["cat"];
        assert!(
            copy_via_pipe(cmd, "test clipboard content"),
            "copy_via_pipe should succeed with {:?}",
            cmd
        );
    }
}
