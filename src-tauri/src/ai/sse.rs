//! Minimal server-sent-events reader over `reqwest::Response::chunk()` (no `stream` feature needed).

use anyhow::Result;

/// Accumulates bytes and yields the `data:` payload of each event.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: String,
}

impl SseParser {
    /// Feed bytes; calls `on_data` once per complete event.
    pub fn feed<F: FnMut(&str) -> Result<()>>(&mut self, bytes: &[u8], on_data: &mut F) -> Result<()> {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&self.buf[..pos]).trim_end_matches('\r').to_string();
            self.buf.drain(..=pos);
            self.line(&line, on_data)?;
        }
        Ok(())
    }

    /// Flush whatever is left (a final event without a trailing blank line).
    pub fn finish<F: FnMut(&str) -> Result<()>>(&mut self, on_data: &mut F) -> Result<()> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf).trim_end_matches('\r').to_string();
            self.buf.clear();
            self.line(&line, on_data)?;
        }
        if !self.data.is_empty() {
            let d = std::mem::take(&mut self.data);
            on_data(&d)?;
        }
        Ok(())
    }

    fn line<F: FnMut(&str) -> Result<()>>(&mut self, line: &str, on_data: &mut F) -> Result<()> {
        if line.is_empty() {
            if !self.data.is_empty() {
                let d = std::mem::take(&mut self.data);
                on_data(&d)?;
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `event:`, `id:`, `retry:` and comments are ignored; providers put everything in `data`.
        Ok(())
    }
}

/// Read a streaming response to the end, calling `on_data` with each event's payload.
pub async fn read_sse<F: FnMut(&str) -> Result<()>>(mut resp: reqwest::Response, mut on_data: F) -> Result<()> {
    let mut p = SseParser::default();
    while let Some(chunk) = resp.chunk().await? {
        p.feed(&chunk, &mut on_data)?;
    }
    p.finish(&mut on_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_chunks() {
        let mut p = SseParser::default();
        let mut got = vec![];
        let mut f = |d: &str| {
            got.push(d.to_string());
            Ok(())
        };
        p.feed(b"event: x\r\ndata: {\"a\":", &mut f).unwrap();
        p.feed(b" 1}\r\n\r\ndata: one\ndata: two\n\n: comment\ndata: [DONE]", &mut f).unwrap();
        p.finish(&mut f).unwrap();
        assert_eq!(got, vec!["{\"a\": 1}", "one\ntwo", "[DONE]"]);
    }
}
