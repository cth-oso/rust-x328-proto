mod common;

use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use x328_proto::master::io::Master;
use x328_proto::node::{self, BusNode};
use x328_proto::{master, Value};

use common::{BusInterface, RS422Bus};

fn master_main_loop(io: BusInterface) -> Result<(), master::io::Error> {
    let mut master = Master::new(io);

    for _ in 1..4 {
        for addr in 5..6 {
            println!("master send write");
            match master.write_parameter(addr, 20, (30 + addr) as i32) {
                Ok(()) => println!("master: write ok"),
                Err(err) => println!("master: write error {:?}", err),
            }

            match master.read_parameter(addr, 20) {
                Ok(val) => println!("Master read param ok {}", *val),
                Err(err) => println!("Master read error {:?}", err),
            }
        }
    }
    // test read again
    master.set_can_read_again(5, true);
    for _ in 0..10 {
        assert!(master.read_parameter(5, 25).is_ok());
    }
    println!("Master terminating");
    Ok(())
}

fn node_main_loop(mut serial: BusInterface) -> Result<(), node::Error> {
    let mut node = BusNode::new(5)?;
    'main: loop {
        if SHUTDOWN.load(SeqCst) {
            break 'main;
        };

        node = match node {
            BusNode::ReceiveData(recv) => {
                let mut buf = [0; 1];
                if let Ok(len) = serial.read(&mut buf) {
                    if len == 0 {
                        println!("Short read");
                        break 'main;
                    }
                    recv.receive_data(&buf[..len])
                } else {
                    println!("Node read error");
                    break 'main;
                }
            }

            BusNode::SendData(mut send) => {
                println!("Node sending data");
                serial.write_all(send.send_data().as_ref()).unwrap();
                send.data_sent()
            }

            BusNode::ReadParameter(read_command) => {
                if read_command.parameter() == 3 {
                    read_command.send_invalid_parameter()
                } else {
                    read_command.send_reply_ok(Value::new(4).unwrap())
                }
            }

            BusNode::WriteParameter(write_command) => {
                let param = write_command.parameter();
                println!("Write to parameter {:?}", param);
                write_command.write_ok()
            }
        };
    }
    println!("Node terminating");
    Ok(())
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
    println!("Master joined");

    SHUTDOWN.store(true, SeqCst);
    bus.wake_blocked_nodes();

    node.join()
        .expect("Node paniced")
        .expect("Node returned an error");
    println!("Node joined")
}
