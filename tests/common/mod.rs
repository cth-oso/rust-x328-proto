#![allow(dead_code)]

use std::cell::RefCell;
use std::cmp::min;
use std::io::{Error, ErrorKind};
use std::rc::Rc;

pub mod sync;

pub mod bytes {
    pub const STX: u8 = 2;
    pub const ETX: u8 = 3;
    pub const ACK: u8 = 6;
    pub const NAK: u8 = 21;
}

pub struct SerialInterface {
    rx: Vec<u8>,
    rx_pos: usize,
    tx: Vec<u8>,
    do_read_error: bool,
    do_write_error: bool,
}

pub struct SerialIOPlane(Rc<RefCell<SerialInterface>>);

impl SerialIOPlane {
    pub fn new(serial_if: &Rc<RefCell<SerialInterface>>) -> SerialIOPlane {
        SerialIOPlane(serial_if.clone())
    }
}

impl SerialInterface {
    pub fn new(rx: &[u8]) -> Rc<RefCell<SerialInterface>> {
        Rc::new(RefCell::new(SerialInterface {
            rx: rx.to_vec(),
            tx: Vec::new(),
            rx_pos: 0,
            do_read_error: false,
            do_write_error: false,
        }))
    }

    pub fn trigger_write_error(&mut self) {
        self.do_write_error = true;
    }

    pub fn trigger_read_error(&mut self) {
        self.do_read_error = true;
    }
}

impl std::io::Read for SerialIOPlane {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut inner = self.0.borrow_mut();
        if inner.do_read_error {
            inner.do_read_error = false;
            Err(Error::new(ErrorKind::PermissionDenied, "IO read error"))
        } else {
            let old_pos = inner.rx_pos;
            inner.rx_pos = min(old_pos + buf.len(), inner.rx.len());
            let len = inner.rx_pos - old_pos;
            buf[..len].copy_from_slice(&inner.rx[old_pos..inner.rx_pos]);
            Ok(len)
        }
    }
}

impl std::io::Write for SerialIOPlane {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut inner = self.0.borrow_mut();
        if inner.do_write_error {
            inner.do_write_error = false;
            Err(Error::new(ErrorKind::PermissionDenied, "IO write error"))
        } else {
            inner.tx.write(buf)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
