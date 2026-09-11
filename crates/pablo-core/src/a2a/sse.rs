//! Incremental bounded SSE framing. Reconnection directives carry no authority.
use super::wire::{Error, MAX_RESPONSE_BYTES, MAX_STREAM_BYTES, MAX_STREAM_UPDATES};

#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    frame_bytes: usize,
    total: usize,
    frames: usize,
    skip_lf: bool,
    failed: bool,
    first_line_seen: bool,
}
impl Decoder {
    /// Feed arbitrary HTTP chunk boundaries byte by byte. Raw comments/fields and
    /// blank frames count toward bounds, even when they produce no JSON payload.
    pub fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, Error> {
        if self.failed {
            return Err(Error::Invalid);
        }
        let result = self.accept(byte);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn accept(&mut self, byte: u8) -> Result<Option<Vec<u8>>, Error> {
        self.total += 1;
        self.frame_bytes += 1;
        // Allow bounded SSE field syntax around a maximum JSONRPC envelope.
        if self.total > MAX_STREAM_BYTES || self.frame_bytes > MAX_RESPONSE_BYTES + 4096 {
            return Err(Error::Bound);
        }
        if self.skip_lf && byte == b'\n' {
            self.skip_lf = false;
            return Ok(None);
        }
        self.skip_lf = byte == b'\r';
        if !matches!(byte, b'\r' | b'\n') {
            self.line.push(byte);
            return Ok(None);
        }
        if !self.first_line_seen {
            self.first_line_seen = true;
            if self.line.starts_with(&[0xef, 0xbb, 0xbf]) {
                self.line.drain(..3);
            }
        }
        if self.line.is_empty() {
            self.frame_bytes = 0;
            self.frames += 1;
            if self.frames > MAX_STREAM_UPDATES {
                return Err(Error::Bound);
            }
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop();
            return Ok(Some(std::mem::take(&mut self.data)));
        }
        if self.line == b"data" || self.line.starts_with(b"data:") {
            let value = self.line.get(5..).unwrap_or_default();
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if value.len() + 1 > (MAX_RESPONSE_BYTES + 1).saturating_sub(self.data.len()) {
                return Err(Error::Bound);
            }
            self.data.extend_from_slice(value);
            self.data.push(b'\n');
        }
        self.line.clear();
        Ok(None)
    }
    /// A truncated data frame cannot become an accepted final response at EOF.
    pub fn finish(self) -> Result<(), Error> {
        if self.failed || !self.line.is_empty() || !self.data.is_empty() {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_boundaries_line_endings_and_multiline_json() {
        for ending in ["\n", "\r", "\r\n"] {
            let input = [
                ": comment",
                "retry: 1",
                "id: no-reconnect",
                "event: message",
                "data: {",
                "data: \"value\":\"雪\"}",
                "",
                "",
            ]
            .join(ending);
            let input = format!("\u{feff}{input}");
            let mut parser = Decoder::default();
            let mut output = Vec::new();
            for byte in input.bytes() {
                if let Some(frame) = parser.push(byte).unwrap() {
                    output.push(frame);
                }
            }
            parser.finish().unwrap();
            assert_eq!(output, vec!["{\n\"value\":\"雪\"}".as_bytes()]);
        }
    }
    #[test]
    fn raw_comments_empty_frames_and_unterminated_lines_are_bounded() {
        let mut parser = Decoder::default();
        for _ in 0..MAX_RESPONSE_BYTES + 4096 {
            parser.push(b'x').unwrap();
        }
        assert_eq!(parser.push(b'x'), Err(Error::Bound));
        assert!(parser.finish().is_err());
        let mut parser = Decoder::default();
        for _ in 0..MAX_STREAM_UPDATES {
            parser.push(b'\n').unwrap();
        }
        assert_eq!(parser.push(b'\n'), Err(Error::Bound));
        let mut parser = Decoder::default();
        for byte in b"data: {}\n" {
            parser.push(*byte).unwrap();
        }
        assert!(parser.finish().is_err());
    }
    #[test]
    fn payload_limit_and_total_raw_limit_are_independent() {
        let mut parser = Decoder::default();
        for byte in b"data: " {
            parser.push(*byte).unwrap();
        }
        for _ in 0..MAX_RESPONSE_BYTES {
            parser.push(b'x').unwrap();
        }
        parser.push(b'\n').unwrap();
        assert_eq!(
            parser.push(b'\n').unwrap().unwrap().len(),
            MAX_RESPONSE_BYTES
        );
        parser.finish().unwrap();
        let mut parser = Decoder::default();
        let frame = format!(": {}\n\n", "x".repeat(65530));
        let mut rejected = false;
        for byte in frame.bytes().cycle().take(MAX_STREAM_BYTES + 1) {
            if parser.push(byte).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected);
    }
}
