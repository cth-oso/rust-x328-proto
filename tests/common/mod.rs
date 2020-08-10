#![allow(dead_code)]

use nom::lib::std::collections::VecDeque;
use std::cell::RefCell;
use std::cmp::min;
use std::io::{Error, ErrorKind};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;
use x328_proto::X328Error;

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
            Err(Error::new(ErrorKind::PermissionDenied, X328Error::IOError))
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
            Err(Error::new(ErrorKind::PermissionDenied, X328Error::IOError))
        } else {
            inner.tx.write(buf)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type BusT = Arc<Mutex<VecDeque<u8>>>;

#[derive(Default)]
pub struct RS422Bus {
    masters: Mutex<Vec<Weak<BusInterfaceLink>>>,
    slaves: Mutex<Vec<Weak<BusInterfaceLink>>>,
    master_data_available: Arc<Condvar>,
    slave_data_available: Arc<Condvar>,
}

impl RS422Bus {
    pub fn new() -> Arc<RS422Bus> {
        Default::default()
    }

    pub fn new_master_interface(self: &Arc<Self>) -> BusInterface {
        let link = Arc::new(BusInterfaceLink {
            is_master: true,
            rx: Default::default(),
            rx_condvar: Arc::clone(&self.master_data_available),
        });
        self.masters.lock().unwrap().push(Arc::downgrade(&link));
        BusInterface::new(Arc::clone(self), link)
    }

    pub fn new_slave_interface(self: &Arc<RS422Bus>) -> BusInterface {
        let link = Arc::new(BusInterfaceLink {
            is_master: false,
            rx: Default::default(),
            rx_condvar: Arc::clone(&self.slave_data_available),
        });
        self.slaves.lock().unwrap().push(Arc::downgrade(&link));
        BusInterface::new(Arc::clone(&self), link)
    }

    pub fn wake_blocked_slaves(&self) {
        self.slave_data_available.notify_all()
    }

    fn send_to_slaves(self: &Arc<Self>, data: u8) {
        let slaves = self.slaves.lock().unwrap();
        for weak in slaves.iter() {
            if let Some(slave) = weak.upgrade() {
                slave.rx.lock().unwrap().push_back(data);
            }
            self.slave_data_available.notify_all();
        }
    }

    fn send_to_masters(self: &Arc<Self>, data: u8) {
        let masters = self.masters.lock().unwrap();
        for weak in masters.iter() {
            if let Some(master) = weak.upgrade() {
                master.rx.lock().unwrap().push_back(data);
            }
            self.master_data_available.notify_all();
        }
    }
}

pub struct BusInterface {
    bus: Arc<RS422Bus>,
    link: Arc<BusInterfaceLink>,
    pub blocking_read: bool,
    pub timeout: Duration,
    pub do_read_error: bool,
    pub do_write_error: bool,
}

struct BusInterfaceLink {
    is_master: bool,
    rx: BusT,
    rx_condvar: Arc<Condvar>,
}

impl BusInterface {
    fn new(bus: Arc<RS422Bus>, link: Arc<BusInterfaceLink>) -> BusInterface {
        BusInterface {
            bus,
            link,
            blocking_read: true,
            timeout: Duration::from_secs(1),
            do_read_error: false,
            do_write_error: false,
        }
    }
}

impl std::io::Read for BusInterface {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.do_read_error {
            self.do_read_error = false;
            return Err(Error::new(ErrorKind::PermissionDenied, X328Error::IOError));
        }

        let mut rx = if self.blocking_read {
            self.link.rx.lock().expect("Read mutex is poisoned")
        } else {
            self.link
                .rx
                .try_lock()
                .map_err(|_| Error::new(ErrorKind::WouldBlock, X328Error::IOError))?
        };

        loop {
            match rx.pop_front() {
                Some(byte) => {
                    buf[0] = byte;
                    return Ok(1);
                }
                None => {
                    if self.blocking_read {
                        let x = self
                            .link
                            .rx_condvar
                            .wait_timeout(rx, self.timeout)
                            .expect("Mutex lock failed");
                        rx = x.0;
                        if rx.is_empty() {
                            return Err(Error::new(ErrorKind::TimedOut, X328Error::IOError));
                        }
                    } else {
                        return Ok(0);
                    }
                }
            }
        }
    }
}

impl std::io::Write for BusInterface {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.do_write_error {
            self.do_write_error = false;
            Err(Error::new(ErrorKind::PermissionDenied, X328Error::IOError))
        } else {
            for byte in buf {
                if self.link.is_master {
                    self.bus.send_to_slaves(*byte);
                } else {
                    self.bus.send_to_masters(*byte)
                }
            }
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
