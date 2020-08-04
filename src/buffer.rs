#[derive(Debug)]
pub struct Buffer {
    data: Vec<u8>,
    read_pos: usize,
}

impl Buffer {
    pub fn new() -> Buffer {
        Buffer {
            data: Vec::with_capacity(100),
            read_pos: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len() - self.read_pos
    }

    pub fn as_str_slice(&self) -> &str {
        std::str::from_utf8(&self.data[self.read_pos..]).unwrap()
    }

    pub fn consume(&mut self, len: usize) {
        assert!(len <= self.len());
        self.read_pos += len;
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.read_pos == self.data.len() {
            self.clear();
        }
        let write_pos = self.data.len();
        self.data.extend_from_slice(bytes);
        for byte in self.data[write_pos..].iter_mut() {
            if *byte > 0x7f {
                *byte = 0; // map all non-ASCII bytes to NUL
            }
        }
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.read_pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_buffer() -> Buffer {
        let mut buf = Buffer::new();
        buf.write(b"abcdabcdabcd");
        buf
    }

    #[test]
    fn test_slice() {
        let buf = get_buffer();
        // print!("{:#?}", buf);
        assert_eq!(buf.as_str_slice().len(), buf.len());
    }
}
