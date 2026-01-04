use std::collections::VecDeque;

#[derive(Debug)]
pub struct OutputBuffer {
    data: VecDeque<u8>,
    start_cursor: u64,
    max_bytes: usize,
    max_lines: usize,
    line_count: usize,
    dropped_bytes_total: u64,
}

#[derive(Debug, Clone)]
pub struct BufferSlice {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub dropped_bytes: u64,
    pub start_cursor: u64,
    pub end_cursor: u64,
    pub buffered_bytes: usize,
    pub buffer_limit_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct TailSlice {
    pub bytes: Vec<u8>,
    pub start_cursor: u64,
    pub end_cursor: u64,
    pub truncated: bool,
    pub buffered_bytes: usize,
    pub buffer_limit_bytes: usize,
}

impl OutputBuffer {
    pub fn new(max_bytes: usize, max_lines: usize) -> Self {
        Self {
            data: VecDeque::new(),
            start_cursor: 0,
            max_bytes: max_bytes.max(1),
            max_lines: max_lines.max(1),
            line_count: 0,
            dropped_bytes_total: 0,
        }
    }

    pub fn append(&mut self, bytes: &[u8]) -> u64 {
        for &byte in bytes {
            if byte == b'\n' {
                self.line_count = self.line_count.saturating_add(1);
            }
            self.data.push_back(byte);
        }
        
        self.enforce_limits()
    }

    pub fn buffer_start(&self) -> u64 {
        self.start_cursor
    }

    pub fn buffer_end(&self) -> u64 {
        self.start_cursor + self.data.len() as u64
    }

    pub fn buffered_bytes(&self) -> usize {
        self.data.len()
    }

    pub fn slice_from(&self, cursor: u64, max_bytes: usize) -> BufferSlice {
        let end_cursor = self.buffer_end();
        if self.data.is_empty() {
            return BufferSlice {
                bytes: Vec::new(),
                truncated: cursor < self.start_cursor,
                dropped_bytes: self.start_cursor.saturating_sub(cursor),
                start_cursor: self.start_cursor,
                end_cursor,
                buffered_bytes: 0,
                buffer_limit_bytes: self.max_bytes,
            };
        }

        let mut truncated = false;
        let mut dropped_bytes = 0;
        let mut effective_cursor = cursor;
        if cursor < self.start_cursor {
            truncated = true;
            dropped_bytes = self.start_cursor - cursor;
            effective_cursor = self.start_cursor;
        }
        if effective_cursor > end_cursor {
            effective_cursor = end_cursor;
        }
        let start_index = (effective_cursor - self.start_cursor) as usize;
        let available = self.data.len().saturating_sub(start_index);
        let read_len = available.min(max_bytes);
        let mut bytes = Vec::with_capacity(read_len);
        for i in 0..read_len {
            if let Some(value) = self.data.get(start_index + i) {
                bytes.push(*value);
            }
        }

        BufferSlice {
            bytes,
            truncated,
            dropped_bytes,
            start_cursor: self.start_cursor,
            end_cursor,
            buffered_bytes: self.data.len(),
            buffer_limit_bytes: self.max_bytes,
        }
    }

    pub fn tail(&self, max_bytes: usize, max_lines: Option<usize>) -> TailSlice {
        let end_cursor = self.buffer_end();
        if self.data.is_empty() {
            return TailSlice {
                bytes: Vec::new(),
                start_cursor: self.start_cursor,
                end_cursor,
                truncated: false,
                buffered_bytes: 0,
                buffer_limit_bytes: self.max_bytes,
            };
        }

        let mut start_index = 0usize;
        if let Some(max_lines) = max_lines {
            let mut lines = 0usize;
            for (idx, &byte) in self.data.iter().enumerate().rev() {
                if byte == b'\n' {
                    lines += 1;
                    if lines > max_lines {
                        start_index = idx + 1;
                        break;
                    }
                }
            }
        }
        let available = self.data.len().saturating_sub(start_index);
        let bytes_to_take = available.min(max_bytes);
        if bytes_to_take < available {
            start_index = self.data.len().saturating_sub(bytes_to_take);
        }
        let mut bytes = Vec::with_capacity(bytes_to_take);
        for i in 0..bytes_to_take {
            if let Some(value) = self.data.get(start_index + i) {
                bytes.push(*value);
            }
        }
        let start_cursor = self.start_cursor + start_index as u64;
        let truncated = bytes_to_take < self.data.len();

        TailSlice {
            bytes,
            start_cursor,
            end_cursor,
            truncated,
            buffered_bytes: self.data.len(),
            buffer_limit_bytes: self.max_bytes,
        }
    }

    fn enforce_limits(&mut self) -> u64 {
        let mut dropped = 0u64;
        while self.data.len() > self.max_bytes {
            if let Some(byte) = self.data.pop_front() {
                dropped += 1;
                self.start_cursor += 1;
                if byte == b'\n' {
                    self.line_count = self.line_count.saturating_sub(1);
                }
            }
        }
        while self.line_count > self.max_lines {
            if let Some(byte) = self.data.pop_front() {
                dropped += 1;
                self.start_cursor += 1;
                if byte == b'\n' {
                    self.line_count = self.line_count.saturating_sub(1);
                }
            } else {
                break;
            }
        }
        if dropped > 0 {
            self.dropped_bytes_total = self.dropped_bytes_total.saturating_add(dropped);
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_drops_oldest_on_max_bytes() {
        let mut buffer = OutputBuffer::new(5, 10);
        buffer.append(b"hello");
        buffer.append(b"world");

        assert_eq!(buffer.buffer_start(), 5);
        assert_eq!(buffer.buffer_end(), 10);
        let slice = buffer.slice_from(0, 10);
        assert!(slice.truncated);
        assert_eq!(slice.dropped_bytes, 5);
        assert_eq!(slice.bytes, b"world");
    }

    #[test]
    fn buffer_limits_lines() {
        let mut buffer = OutputBuffer::new(100, 2);
        buffer.append(b"a\nb\nc\n");
        let slice = buffer.slice_from(buffer.buffer_start(), 100);
        assert_eq!(slice.bytes, b"b\nc\n");
    }

    #[test]
    fn tail_respects_max_lines() {
        let mut buffer = OutputBuffer::new(100, 10);
        buffer.append(b"line1\nline2\nline3\n");
        let tail = buffer.tail(100, Some(1));
        assert_eq!(tail.bytes, b"line3\n");
    }
}
