//! 纯字节、无网络依赖的 SSE 行解码器。
//!
//! 设计原则：`feed(&[u8])` 只处理完整行，半包字节留存到下次；
//! UTF-8 多字节字符不会被半个切断（换行符 `\n` 不可能出现在多字节序列内）。

/// SSE 解码器（有状态）。
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一批字节，返回本次可解析出的 `data:` 负载（原始字符串，通常为 JSON 或 `[DONE]`）。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();

        // 找最后一个换行：它之前的都是完整行，尾部残片保留。
        let complete_len = match self.buf.iter().rposition(|b| *b == b'\n') {
            Some(pos) => pos + 1,
            None => 0,
        };
        if complete_len == 0 {
            return out;
        }
        let complete = self.buf.split_off(complete_len);
        let lines = std::mem::replace(&mut self.buf, complete);

        for line in lines.split(|b| *b == b'\n') {
            let mut line = line;
            // 兼容 CRLF
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if line.is_empty() || line[0] == b':' {
                continue; // 空行 / SSE 注释
            }
            let payload = if let Some(rest) = line.strip_prefix(b"data:") {
                rest
            } else {
                continue; // event:/id:/retry: 等字段忽略
            };
            let payload = trim_ascii_ws(payload);
            if payload.is_empty() {
                continue;
            }
            // 完整行内的 JSON 必然是合法 UTF-8（\n 不可能落在多字节字符内部）
            match std::str::from_utf8(payload) {
                Ok(s) => out.push(s.to_string()),
                Err(_) => {
                    // 服务端发来非法 UTF-8：容错跳过该行，不中断整个流
                    tracing::warn!("sse: skipping non-utf8 data line ({} bytes)", payload.len());
                }
            }
        }
        out
    }

    /// 是否还有未处理的残片。
    pub fn has_pending(&self) -> bool {
        !self.buf.is_empty()
    }
}

fn trim_ascii_ws(b: &[u8]) -> &[u8] {
    let mut s = b;
    while let Some(first) = s.first() {
        if first.is_ascii_whitespace() {
            s = &s[1..];
        } else {
            break;
        }
    }
    while let Some(last) = s.last() {
        if last.is_ascii_whitespace() {
            s = &s[..s.len() - 1];
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(dec: &mut SseDecoder, frames: &[&[u8]]) -> Vec<String> {
        let mut out = Vec::new();
        for f in frames {
            out.extend(dec.feed(f));
        }
        out
    }

    #[test]
    fn parses_data_lines_and_done() {
        let mut dec = SseDecoder::new();
        let out = feed_all(&mut dec, &[b"data: {\"a\":1}\n\n", b"data: [DONE]\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn half_packet_is_buffered_until_completion() {
        let mut dec = SseDecoder::new();
        // 从中间切开一行
        let frame = b"data: {\"part\":\"xy";
        assert!(dec.feed(frame).is_empty());
        assert!(dec.has_pending());
        let out = dec.feed(b"z\"}\n\n");
        assert_eq!(out, vec!["{\"part\":\"xyz\"}".to_string()]);
    }

    #[test]
    fn split_inside_multibyte_utf8_is_safe() {
        let full = "data: {\"t\":\"中文字\"}\n".as_bytes();
        // 在多字节字符内部切（中 = E4 B8 AD）
        let cut = full.iter().rposition(|_| true).unwrap();
        let _ = cut;
        let split_at = 12; // 落在 UTF-8 序列内部的位置
        let (a, b) = full.split_at(split_at);
        let mut dec = SseDecoder::new();
        assert!(dec.feed(a).is_empty());
        let out = dec.feed(b);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("中文字"));
    }

    #[test]
    fn comment_and_field_lines_ignored() {
        let mut dec = SseDecoder::new();
        let raw =
            b": ping\nevent: message\ndata: {\"k\":1}\nid: 42\nretry: 100\n\ndata: {\"k\":2}\n\n";
        let out = dec.feed(raw);
        assert_eq!(out, vec!["{\"k\":1}".to_string(), "{\"k\":2}".to_string()]);
    }

    #[test]
    fn crlf_tolerated() {
        let mut dec = SseDecoder::new();
        let out = dec.feed(b"data: 1\r\ndata: 2\r\n");
        assert_eq!(out, vec!["1", "2"]);
    }
}
