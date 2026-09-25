//! 剪贴板复制（M4-6）。
//!
//! 默认走 OSC 52 转义序列（纯协议、无依赖、SSH 友好）；
//! 可选 `clipboard` feature 用 arboard 走系统剪贴板，失败静默降级到 OSC 52。

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
    copy_osc52(text)
}

/// OSC 52 复制：`ESC ] 52 ; c ; <base64> BEL`。
fn copy_osc52(text: &str) -> bool {
    use std::io::Write;
    let encoded = base64(text.as_bytes());
    let seq = format!("\u{1b}]52;c;{encoded}\u{07}");
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
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
    use super::base64;

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
}
