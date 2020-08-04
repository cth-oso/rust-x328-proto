mod common;
use common::{BusInterface, RS422Bus};

use std::convert::TryInto;
use std::io::{Read, Write};
use std::thread;
use x328_proto::{master::io::Master, slave::Slave, X328Error};

fn master_main_loop(io: BusInterface) -> Result<(), X328Error> {
    let mut master = Master::new(io);
    for _ in 1..4 {
        for addr in 5..6 {
            println!("master send write");
            let x = master.write_parameter(addr.try_into()?, 20.try_into()?, 30 + addr as i32)?;
            println!("master write response {:?}", x);
        }
    }
    Ok(())
}

fn slave_loop(mut serial: BusInterface) -> Result<(), X328Error> {
    let mut slave_proto = Slave::new(5.try_into()?);
    'main: loop {
        slave_proto = match slave_proto {
            Slave::ReadData(recv) => {
                let mut buf = [0; 1];
                if let Ok(len) = serial.read(&mut buf) {
                    if len == 0 {
                        break 'main;
                    }
                    recv.receive_data(&buf[..len])
                } else {
                    break 'main;
                }
            }

            Slave::SendData(mut send) => {
                serial.write_all(send.send_data().as_ref()).unwrap();
                send.data_sent()
            }

            Slave::ReadParameter(read_command) => {
                if read_command.get_parameter() == 3 {
                    read_command.send_invalid_parameter()
                } else {
                    read_command.send_reply_ok(4)
                }
            }

            Slave::WriteParameter(write_command) => {
                let param = write_command.get_parameter();
                println!("Write to parameter {:?}", param);
                write_command.write_ok()
            }
        };
    }
    Ok(())
}

#[test]
fn chat1() {
    let bus = RS422Bus::new();
    let master_if = bus.new_master_interface();
    let master = thread::spawn(move || master_main_loop(master_if));
    let slave_if = bus.new_slave_interface();
    let slave = thread::spawn(move || slave_loop(slave_if));

    println!("Threads running");
    master
        .join()
        .expect("Join failed")
        .expect("Master returned an error");
    println!("Master joined");
    slave
        .join()
        .expect("Slave paniced")
        .expect("Slave returned an error");
    println!("Slave joined")
}
