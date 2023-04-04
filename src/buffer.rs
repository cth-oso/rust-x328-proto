use arrayvec::ArrayVec;

const DEFAULT_BUF_SIZE: usize = 40; // The maximum X3.28 message length is 18 bytes

#[derive(Debug)]
pub struct Buffer<const BUF_SIZE: usize = DEFAULT_BUF_SIZE> {
    data: ArrayVec<u8, BUF_SIZE>,
    read_pos: usize,
}

impl<const BUF_SIZE: usize> Buffer<BUF_SIZE> {
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

    pub fn push(&mut self, byte: u8) {
        if self.data.is_full() {
            // Run the data shifting logic in self.write()
            self.write(&[byte])
        } else {
            self.data.push(byte)
        }
    }

    pub fn write(&mut self, mut bytes: &[u8]) {
        if self.read_pos == self.data.len() {
            self.clear();
        }
        if bytes.len() > self.data.capacity() {
            bytes = &bytes[(bytes.len() - self.data.capacity())..];
            self.clear();
        } else {
            let cap = self.data.remaining_capacity();
            if cap < bytes.len() {
                let drain_len = bytes.len() - cap;
                self.data.drain(..drain_len);
                self.read_pos = self.read_pos.saturating_sub(drain_len);
            }
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

    pub fn get_ref_and_clear(&mut self) -> &[u8] {
        let pos = self.read_pos;
        self.consume(self.len());
        &self.data[pos..]
    }
}

impl<const BUF_SIZE: usize> AsRef<[u8]> for Buffer<BUF_SIZE> {
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

    #[test]
    fn buffer_spill() {
        let mut buf = Buffer::<DEFAULT_BUF_SIZE>::new();
        for _ in 0..(DEFAULT_BUF_SIZE + 1) {
            buf.write(b"a");
        }
        assert_eq!(buf.read_pos, 0);
        buf.consume(5);
        assert_eq!(buf.read_pos, 5);
        buf.write(b"1234");
        assert_eq!(buf.read_pos, 1);
        buf.write(b"5");
        assert_eq!(buf.read_pos, 0);
        buf.write(b"67");
        assert_eq!(buf.read_pos, 0);
    }

    #[test]
    fn too_large_write() {
        let mut buf = Buffer::<DEFAULT_BUF_SIZE>::new();
        let data: String = std::iter::once("abc")
            .cycle()
            .take(DEFAULT_BUF_SIZE)
            .collect();
        buf.write(data.as_bytes());
        assert_eq!(buf.data, data.as_bytes()[(data.len() - DEFAULT_BUF_SIZE)..])
    }
}
