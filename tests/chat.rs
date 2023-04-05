use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use common::{BusInterface, RS422Bus};
use x328_proto::master;
use x328_proto::master::io::Master;
use x328_proto::node::{Node, ParamRequest};
use x328_proto::{addr, NodeState};

mod common;

fn master_main_loop(io: BusInterface) -> Result<(), master::io::Error> {
    let mut master = Master::new(io);

    for _ in 1..4 {
        for addr in 5..6 {
            master.write_parameter(addr, 20, (30 + addr) as i32)?;
            master.read_parameter(addr, 20)?;
        }
    }
    // test read again
    for _ in 0..10 {
        assert!(master.read_parameter_again(5, 25).is_ok());
    }
    Ok(())
}

fn node_main_loop(mut serial: BusInterface) {
    let mut node = Node::new(addr(5));
    'main: loop {
        if SHUTDOWN.load(SeqCst) {
            break 'main;
        };

        match node.state() {
            NodeState::ReceiveData(recv) => {
                let mut buf = [0; 1];
                if let Ok(len) = serial.read(&mut buf) {
                    if len == 0 {
                        break 'main;
                    }
                    recv.receive_data(&buf[..len]);
                } else {
                    break 'main;
                }
            }

            NodeState::SendData(send) => {
                serial.write_all(send.send_data()).unwrap();
            }

            NodeState::Command(cmd) => match cmd {
                ParamRequest::Read(read_command) => {
                    if read_command.parameter() == 3 {
                        read_command.send_invalid_parameter();
                    } else {
                        read_command.send_reply_ok(4u16.into());
                    }
                }
                ParamRequest::Write(write_command) => {
                    write_command.write_ok();
                }
            },
        };
    }
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[test]
fn chat1() {
    SHUTDOWN.store(false, SeqCst);

    let bus = RS422Bus::new();
    let mut master_if = bus.new_master_interface();
    master_if.timeout = Duration::from_millis(100);

    let mut node_if = bus.new_node_interface();
    node_if.timeout = Duration::from_secs(100);
    let master = thread::spawn(move || master_main_loop(master_if));
    let node = thread::spawn(move || node_main_loop(node_if));

    master
        .join()
        .expect("Join failed")
        .expect("Master returned an error");

    SHUTDOWN.store(true, SeqCst);
    bus.wake_blocked_nodes();

    node.join().expect("Node panicked");
}
