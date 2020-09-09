mod common;

use std::convert::TryInto;
use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use x328_proto::master;
use x328_proto::master::io::Master;
use x328_proto::slave::{self, Slave};

use common::{BusInterface, RS422Bus};

fn master_main_loop(io: BusInterface) -> Result<(), master::io::Error> {
    let mut master = Master::new(io);

    for _ in 1..4 {
        for addr in 5..6 {
            println!("master send write");
            let a = addr.try_into().unwrap();
            match master.write_parameter(a, 20.try_into()?, 30 + addr as i32) {
                Ok(()) => println!("master: write ok"),
                Err(err) => println!("master: write error {:?}", err),
            }

            match master.read_parameter(a, 20.try_into().unwrap()) {
                Ok(val) => println!("Master read param ok {}", val),
                Err(err) => println!("Master read error {:?}", err),
            }
        }
    }
    // test read again
    let a5 = 5.try_into().unwrap();
    master.set_can_read_again(a5, true);
    for _ in 0..10 {
        assert!(master.read_parameter(a5, 25.try_into().unwrap()).is_ok());
    }
    println!("Master terminating");
    Ok(())
}

fn slave_loop(mut serial: BusInterface) -> Result<(), slave::Error> {
    let mut slave_proto = Slave::new(5.try_into()?);
    'main: loop {
        if SHUTDOWN.load(SeqCst) {
            break 'main;
        };

        slave_proto = match slave_proto {
            Slave::ReadData(recv) => {
                let mut buf = [0; 1];
                if let Ok(len) = serial.read(&mut buf) {
                    if len == 0 {
                        println!("Short read");
                        break 'main;
                    }
                    recv.receive_data(&buf[..len])
                } else {
                    println!("Slave read error");
                    break 'main;
                }
            }

            Slave::SendData(mut send) => {
                println!("Slave sending data");
                serial.write_all(send.send_data().as_ref()).unwrap();
                send.data_sent()
            }

            Slave::ReadParameter(read_command) => {
                if read_command.parameter() == 3 {
                    read_command.send_invalid_parameter()
                } else {
                    read_command.send_reply_ok(4)
                }
            }

            Slave::WriteParameter(write_command) => {
                let param = write_command.parameter();
                println!("Write to parameter {:?}", param);
                write_command.write_ok()
            }
        };
    }
    println!("Slave terminating");
    Ok(())
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[test]
fn chat1() {
    SHUTDOWN.store(false, SeqCst);

    let bus = RS422Bus::new();
    let mut master_if = bus.new_master_interface();
    master_if.timeout = Duration::from_millis(100);

    let mut slave_if = bus.new_slave_interface();
    slave_if.timeout = Duration::from_secs(100);
    let master = thread::spawn(move || master_main_loop(master_if));
    let slave = thread::spawn(move || slave_loop(slave_if));

    master
        .join()
        .expect("Join failed")
        .expect("Master returned an error");
    println!("Master joined");

    SHUTDOWN.store(true, SeqCst);
    bus.wake_blocked_slaves();

    slave
        .join()
        .expect("Slave paniced")
        .expect("Slave returned an error");
    println!("Slave joined")
}
