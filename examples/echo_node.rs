use std::collections::HashMap;
use std::io::{self, Read, Write};

use std::error::Error;
use x328_proto::{addr, NodeState};

fn node_main_loop() -> Result<(), Box<dyn Error>> {
    let mut registers = HashMap::new();

    let mut node = NodeState::new(addr(10));

    loop {
        node = match node {
            NodeState::ReceiveData(recv) => {
                // print!("Reading one byte from stdin\n");
                let mut data_in = vec![0];
                if io::stdin().read(data_in.as_mut_slice())? == 0 {
                    break;
                }
                recv.receive_data(&data_in)
            }

            NodeState::SendData(mut send) => {
                io::stdout().write_all(send.get_data())?;
                send.data_sent().into()
            }

            NodeState::ReadParameter(read_command) => {
                print!("Received read command {:?}", read_command);
                if read_command.parameter() == 3 {
                    read_command.send_invalid_parameter()
                } else {
                    read_command.send_reply_ok(4u16.into())
                }
                .into()
            }

            NodeState::WriteParameter(write_command) => {
                print!("Received write command at {:?}", write_command);
                let param = write_command.parameter();
                if param == 3 {
                    write_command.write_error()
                } else {
                    registers.insert(param, write_command.value());
                    write_command.write_ok()
                }
                .into()
            }
        };
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    node_main_loop()
}
