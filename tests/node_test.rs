mod common;

use common::{SerialIOPlane, SerialInterface};
use std::collections::HashMap;
use std::io::{Read, Write};
use x328_proto::node::Node;
use x328_proto::{addr, NodeState, Parameter, Value};

#[test]
fn node_main_loop() {
    let data_in = b"asd";
    let serial_sim = SerialInterface::new(data_in);
    let mut serial = SerialIOPlane::new(&serial_sim);
    let mut registers: HashMap<Parameter, Value> = HashMap::new();

    let mut node = Node::new(addr(10));
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
                let param = write_command.parameter();
                token = if param == 3 {
                    write_command.write_error()
                } else {
                    registers.insert(param, write_command.value());
                    write_command.write_ok()
                };
            }
        };
    }
}
