use std::collections::VecDeque;
use std::io::{Error, ErrorKind, Write};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

type BusT = Arc<Mutex<VecDeque<u8>>>;

#[derive(Default)]
pub struct RS422Bus {
    masters: Mutex<Vec<Weak<BusInterfaceLink>>>,
    nodes: Mutex<Vec<Weak<BusInterfaceLink>>>,
    master_data_available: Arc<Condvar>,
    node_data_available: Arc<Condvar>,
    eof: AtomicBool,
}

impl RS422Bus {
    pub fn new() -> Arc<RS422Bus> {
        Default::default()
    }

    pub fn disconnect(&self) {
        self.eof.store(true, SeqCst);
        self.node_data_available.notify_all();
        self.master_data_available.notify_all();
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

    pub fn new_node_interface(self: &Arc<RS422Bus>) -> BusInterface {
        let link = Arc::new(BusInterfaceLink {
            is_master: false,
            rx: Default::default(),
            rx_condvar: Arc::clone(&self.node_data_available),
        });
        self.nodes.lock().unwrap().push(Arc::downgrade(&link));
        BusInterface::new(Arc::clone(&self), link)
    }

    fn send_to_nodes(self: &Arc<Self>, data: u8) {
        let nodes = self.nodes.lock().unwrap();
        for weak in nodes.iter() {
            if let Some(node) = weak.upgrade() {
                node.rx.lock().unwrap().push_back(data);
            }
            self.node_data_available.notify_all();
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
            timeout: Duration::from_millis(100),
            do_read_error: false,
            do_write_error: false,
        }
    }

    pub fn putc(&mut self, byte: u8) {
        self.write(&[byte]).unwrap();
    }
}

impl std::io::Read for BusInterface {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            panic!("Testsuite called read with zero length buffer.")
        }
        if self.do_read_error {
            self.do_read_error = false;
            return Err(Error::new(ErrorKind::PermissionDenied, "IO read error"));
        }

        let mut rx = if self.blocking_read {
            self.link.rx.lock().expect("Read mutex is poisoned")
        } else {
            self.link
                .rx
                .try_lock()
                .map_err(|_| Error::new(ErrorKind::WouldBlock, "IO read error: would block"))?
        };

        if let Some(byte) = rx.pop_front() {
            buf[0] = byte;
            return Ok(1);
        }

        if self.blocking_read {
            let mut rx = self
                .link
                .rx_condvar
                .wait_timeout(rx, self.timeout)
                .expect("Mutex lock failed")
                .0;
            if let Some(byte) = rx.pop_front() {
                buf[0] = byte;
                Ok(1)
            } else if self.bus.eof.load(SeqCst) {
                Ok(0)
            } else {
                Err(Error::new(ErrorKind::TimedOut, "IO read timeout"))
            }
        } else {
            Ok(0)
        }
    }
}

impl std::io::Write for BusInterface {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.do_write_error {
            self.do_write_error = false;
            Err(Error::new(ErrorKind::PermissionDenied, "IO write error"))
        } else {
            for byte in buf {
                if self.link.is_master {
                    self.bus.send_to_nodes(*byte);
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
