//! SSE 增量解码（`app/sse.py`）：喂任意切分的字节块，吐出完整事件，并在事件 / 整条流超限时报错。
//!
//! 与旧版一致的细节（都有快照测试）：
//! - 行结束符 `\r\n` / `\r` / `\n`；块尾的 `\r` 之后若下一块以 `\n` 开头，这个 `\n` 要吞掉；
//! - 只有 `data` 与 `event` 两个字段被保留（`id` / `retry` / 未知字段 / 注释行都丢弃）；
//! - 空行才触发分派，且至少有一行 `data` 才吐事件；`data` 多行用 `\n` 连接；
//! - 整条流的第一行去掉 UTF-8 BOM；
//! - **事件大小按「内容 + 行结束符」字节数累计**：`\r\n` 被切在两块之间时，只数 `\r` 的 1 字节，
//!   所以同一份数据因为切分不同，事件大小可能差 1（旧版的小怪癖，只在刚好卡限额时才有影响）；
//! - EOF 不分派没写完的事件（`finish` 只清空缓冲）。
//!
//! Python 的 `feed` 是生成器：先吐出的事件会被调用方先处理，之后才抛错。这里一次返回「已解出的事件 + 可选错误」，
//! 调用方按顺序先处理事件、再处理错误即可得到相同效果。

use thiserror::Error;

/// `SSEProtocolError`：事件或整条流超过限额。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("SSE 协议错误：事件或流超过大小限制")]
pub struct SseProtocolError;

/// 一个完整事件：`data` 已按 `\n` 连接，`event` 为空表示没有 `event:` 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub data: Vec<u8>,
    pub event: Vec<u8>,
}

/// 一次 `feed` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFeed {
    /// 本块里解出的事件，按顺序。
    pub events: Vec<SseEvent>,
    /// 解到一半超限时的错误（此前的事件仍然有效）。
    pub error: Option<SseProtocolError>,
}

#[derive(Debug, Clone)]
pub struct SseDecoder {
    event_limit: u64,
    stream_limit: u64,
    line: Vec<u8>,
    data: Vec<Vec<u8>>,
    event: Vec<u8>,
    event_bytes: u64,
    total_bytes: u64,
    skip_lf: bool,
    first_line: bool,
}

const BOM: &[u8] = b"\xef\xbb\xbf";

/// 正则 `\r\n?|\n` 的 `finditer(chunk, start)` 的下一个匹配：返回 (起点, 终点)。
fn next_line_end(chunk: &[u8], from: usize) -> Option<(usize, usize)> {
    let rel = chunk[from..]
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')?;
    let at = from + rel;
    let len = if chunk[at] == b'\r' && chunk.get(at + 1) == Some(&b'\n') {
        2
    } else {
        1
    };
    Some((at, at + len))
}

impl SseDecoder {
    pub fn new(event_limit: u64, stream_limit: u64) -> Self {
        Self {
            event_limit,
            stream_limit,
            line: Vec::new(),
            data: Vec::new(),
            event: Vec::new(),
            event_bytes: 0,
            total_bytes: 0,
            skip_lf: false,
            first_line: true,
        }
    }

    /// 喂一块字节。超限后状态保持「出错那一刻」的样子（与旧版一样，继续喂不会自动恢复）。
    pub fn feed(&mut self, chunk: &[u8]) -> SseFeed {
        let mut events = Vec::new();
        let error = self.feed_into(chunk, &mut events).err();
        SseFeed { events, error }
    }

    fn feed_into(
        &mut self,
        chunk: &[u8],
        events: &mut Vec<SseEvent>,
    ) -> Result<(), SseProtocolError> {
        self.total_bytes += chunk.len() as u64;
        if self.total_bytes > self.stream_limit {
            return Err(SseProtocolError);
        }
        let mut start = 0;
        if self.skip_lf && !chunk.is_empty() {
            start = usize::from(chunk[0] == b'\n');
            self.skip_lf = false;
        }
        while let Some((m_start, m_end)) = next_line_end(chunk, start) {
            self.append(&chunk[start..m_start], (m_end - m_start) as u64)?;
            let mut line = std::mem::take(&mut self.line);
            if self.first_line {
                if line.starts_with(BOM) {
                    line.drain(..BOM.len());
                }
                self.first_line = false;
            }
            start = m_end;
            self.skip_lf = chunk[m_start] == b'\r' && m_end - m_start == 1 && start == chunk.len();
            if line.is_empty() {
                let data = std::mem::take(&mut self.data);
                let event = std::mem::take(&mut self.event);
                self.event_bytes = 0;
                if !data.is_empty() {
                    events.push(SseEvent {
                        data: data.join(&b'\n'),
                        event,
                    });
                }
            } else if !line.starts_with(b":") {
                let (name, value) = match line.iter().position(|&b| b == b':') {
                    Some(i) => {
                        let mut v = &line[i + 1..];
                        if v.first() == Some(&b' ') {
                            v = &v[1..];
                        }
                        (&line[..i], v)
                    }
                    None => (&line[..], &line[line.len()..]),
                };
                if name == b"data" {
                    self.data.push(value.to_vec());
                } else if name == b"event" {
                    self.event = value.to_vec();
                }
            }
        }
        self.append(&chunk[start..], 0)
    }

    fn append(&mut self, fragment: &[u8], delimiter_bytes: u64) -> Result<(), SseProtocolError> {
        self.event_bytes += fragment.len() as u64 + delimiter_bytes;
        if self.event_bytes > self.event_limit {
            return Err(SseProtocolError);
        }
        self.line.extend_from_slice(fragment);
        Ok(())
    }

    /// EOF：丢弃没写完的行和事件（不分派）。已累计的总字节数、BOM 状态不重置（与旧版一致）。
    pub fn finish(&mut self) {
        self.line.clear();
        self.data.clear();
        self.event.clear();
        self.event_bytes = 0;
    }
}

/// `encode_sse(data, event)`：`event` 为空就不写 `event:` 行；`data` 的每一行单独写 `data: `。
pub fn encode_sse(data: &[u8], event: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + event.len() + 24);
    if !event.is_empty() {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(event);
        out.push(b'\n');
    }
    for (i, line) in data.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(line);
    }
    out.extend_from_slice(b"\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(data: &str, event: &str) -> SseEvent {
        SseEvent {
            data: data.as_bytes().to_vec(),
            event: event.as_bytes().to_vec(),
        }
    }

    #[test]
    fn basic_and_crlf_split() {
        let mut d = SseDecoder::new(1 << 20, 1 << 30);
        let f = d.feed(b"event: x\r\ndata: a\r");
        assert!(f.events.is_empty() && f.error.is_none());
        let f = d.feed(b"\ndata: b\r\n\r\n");
        assert_eq!(f.events, vec![ev("a\nb", "x")]);
    }

    #[test]
    fn bom_comments_and_unknown_fields() {
        let mut d = SseDecoder::new(1 << 20, 1 << 30);
        let f = d.feed(b"\xef\xbb\xbf: hi\nid: 1\nretry: 5\ndata\n\n\n");
        assert_eq!(f.events, vec![ev("", "")]);
    }

    #[test]
    fn limits() {
        let mut d = SseDecoder::new(4, 100);
        assert!(d.feed(b"data: abc").error.is_some());
        let mut d = SseDecoder::new(100, 5);
        assert!(d.feed(b"abcdef").error.is_some());
    }

    #[test]
    fn eof_does_not_dispatch() {
        let mut d = SseDecoder::new(100, 100);
        assert!(d.feed(b"data: a\n").events.is_empty());
        d.finish();
        assert!(d.feed(b"\n").events.is_empty());
    }

    #[test]
    fn encode_matches_python() {
        assert_eq!(encode_sse(b"a\nb", b"e"), b"event: e\ndata: a\ndata: b\n\n");
        assert_eq!(encode_sse(b"", b""), b"data: \n\n");
    }
}
