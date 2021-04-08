use arrayvec::ArrayVec;

#[derive(Debug)]
pub struct Buffer {
    data: ArrayVec<u8, 40>, // The maximum X3.28 message length is 18 bytes
    read_pos: usize,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            data: ArrayVec::new(),
            read_pos: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len() - self.read_pos
    }

    pub fn consume(&mut self, len: usize) {
        assert!(len <= self.len());
        self.read_pos += len;
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.read_pos == self.data.len() {
            self.clear();
        }
        let cap = self.data.remaining_capacity();
        if cap < bytes.len() {
            self.data.drain(..(bytes.len() - cap));
        }
        let write_pos = self.data.len();
        self.data.try_extend_from_slice(bytes).unwrap();
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

impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.data[self.read_pos..]
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
        assert_eq!(buf.as_ref().len(), buf.len());
    }
}
