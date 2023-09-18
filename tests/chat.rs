use std::io::{Read, Write};
use std::ops::Deref;
use std::time::Duration;

use crate::common::sync::{BusInterface, RS422Bus};
use x328_proto::master::io::Master;
use x328_proto::node::Node;
use x328_proto::scanner::{ControllerEvent, NodeEvent, Scanner};
use x328_proto::{addr, NodeState};
use x328_proto::{master, param, value};

mod common;

fn master_main_loop(
    io: BusInterface,
    commands: &[ControllerEvent],
) -> Result<(), master::io::Error> {
    let mut master = Master::new(io);

    for cmd in commands {
        match cmd {
            ControllerEvent::Read(addr, param) => {
                master.read_parameter(*addr, *param)?;
            }
            ControllerEvent::Write(a, p, v) => {
                master.write_parameter(*a, *p, *v)?;
            }
            ControllerEvent::NodeTimeout => {}
        }
    }
    Ok(())
}

fn node_main_loop(mut serial: BusInterface) {
    let mut node = Node::new(addr(5));
    let mut token = node.reset();

    'main: loop {
        match node.state(token) {
            NodeState::ReceiveData(recv) => {
                let mut buf = [0; 1];
                if let Ok(len) = serial.read(&mut buf) {
                    if len == 0 {
                        break 'main;
                    }
                    token = recv.receive_data(&buf[..len]);
                } else {
                    break 'main;
                }
            }

            NodeState::SendData(send) => {
                serial.write_all(send.send_data()).unwrap();
                token = send.data_sent();
            }

            NodeState::ReadParameter(read_command) => {
                token = if read_command.parameter() == 3 {
                    read_command.send_invalid_parameter()
                } else {
                    read_command.send_reply_ok(4u16.into())
                }
            }

            NodeState::WriteParameter(write_command) => {
                token = write_command.write_ok();
            }
        };
    }
}

#[derive(Debug)]
enum Event {
    Node(NodeEvent),
    Ctrl(ControllerEvent),
}

struct Buffer {
    data: Vec<u8>,
    write_pos: usize,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            write_pos: 0,
        }
    }
    pub fn read_from(&mut self, mut reader: impl Read) -> Result<usize, std::io::Error> {
        self.data.resize(self.write_pos + 40, 0);
        let len = reader.read(&mut self.data[self.write_pos..])?;
        self.write_pos += len;
        Ok(len)
    }

    pub fn consume(&mut self, len: usize) {
        self.data.drain(..len);
        self.write_pos -= len;
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data[..self.write_pos]
    }
}

fn scanner_thread(mut ctrl_rx_if: impl Read, mut node_rx_if: impl Read) -> Vec<Event> {
    let mut scanner = Scanner::new();

    let mut events = Vec::new();
    let mut buf = Buffer::new();
    'main: loop {
        'node_recv: loop {
            match buf.read_from(&mut node_rx_if) {
                Ok(0) => break 'main,
                Ok(_) => {}
                Err(_) => continue 'main,
            };
            while !buf.is_empty() {
                let (consumed, event) = scanner.recv_from_ctrl(&buf);
                buf.consume(consumed);

                if let Some(e) = event {
                    events.push(Event::Ctrl(e));
                    if buf.is_empty() {
                        break 'node_recv;
                    }
                } else {
                    break;
                }
            }
        }
        loop {
            match buf.read_from(&mut ctrl_rx_if) {
                Ok(0) => break 'main,
                Ok(_) => {}
                Err(_) => continue 'main,
            };
            while !buf.is_empty() {
                let (consumed, event) = scanner.recv_from_node(&buf);
                buf.consume(consumed);

                if let Some(e) = event {
                    events.push(Event::Node(e));
                } else {
                    break;
                }
            }
            if buf.is_empty() {
                break;
            }
        }
    }
    events
}

#[test]
fn chat1() {
    let bus = RS422Bus::new();

    let mut master_if = bus.new_master_interface();
    master_if.timeout = Duration::from_millis(100);
    let mut commands = Vec::new();
    for _ in 1..4 {
        for a in 5..6 {
            commands.push(ControllerEvent::Write(
                addr(a),
                param(20),
                value((30 + a) as i32),
            ));
            commands.push(ControllerEvent::Read(addr(a), param(20)));
        }
    }
    // test read again
    for _ in 0..10 {
        commands.push(ControllerEvent::Read(addr(5), param(25)));
    }

    let events = std::thread::scope(|s| {
        let mut node_if = bus.new_node_interface();
        node_if.timeout = Duration::from_millis(1000);
        s.spawn(|| node_main_loop(node_if));

        let ctrl_rx_if = bus.new_master_interface();
        let node_rx_if = bus.new_node_interface();
        let scanner = s.spawn(|| scanner_thread(ctrl_rx_if, node_rx_if));

        master_main_loop(master_if, &commands).expect("Master error");
        bus.disconnect();
        scanner.join().expect("Scanner panicked")
    });

    let mut cmds = commands.iter();
    for e in events {
        // println!("{e:?}");
        match e {
            Event::Node(_) => {}
            Event::Ctrl(ref ev) => {
                assert_eq!(ev, cmds.next().unwrap())
            }
        }
    }
    assert!(cmds.next().is_none())
}
