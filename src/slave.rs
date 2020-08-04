use ascii::AsciiChar;

const ACK: u8 = AsciiChar::ACK.as_byte();
//const BS: u8 = AsciiChar::BackSpace.as_byte();
// const ENQ: u8 = AsciiChar::ENQ.as_byte();
const EOT: u8 = AsciiChar::EOT.as_byte();
const ETX: u8 = AsciiChar::ETX.as_byte();
const NAK: u8 = AsciiChar::NAK.as_byte();
const STX: u8 = AsciiChar::SOX.as_byte();

use crate::buffer::Buffer;
use crate::nom_parser::{self, AddressToken, CommandToken};
pub use crate::{Address, Parameter, Value};

pub type OptionalAddress = Option<Address>;

pub(crate) fn bcc(data: &[u8]) -> u8 {
    let mut checksum: u8 = 0;
    for byte in data {
        checksum ^= *byte;
    }
    if checksum < 0x20 {
        checksum += 0x20;
    }
    checksum
}

#[derive(Debug)]
pub enum Command {
    Read { parameter: Parameter },
    Write,
}

#[derive(Debug)]
pub enum Slave {
    ReadData(ReadData),
    SendData(SendData),
    ReadParameter(ReadParam),
    WriteParameter(WriteParam),
}

impl Slave {
    pub fn new(address: Address) -> Slave {
        ReadData::create(address)
    }
}

type SlaveState = Box<SlaveStateStruct>;

#[derive(Debug)]
struct SlaveStateStruct {
    slave_address: Address,
    last_address: OptionalAddress,
    last_command: Option<Command>,
}

impl SlaveStateStruct {
    fn set_last_command(&mut self, cmd: Command) {
        self.last_command = Some(cmd);
    }
    fn clear_last_command(&mut self) {
        self.last_command.take();
    }

    fn cmd_to_slave_addr(&self) -> bool {
        Some(self.slave_address) == self.last_address
        // NOTE: This doesn't accept the broadcast address 0
    }
}

#[derive(Debug)]
pub struct ReadData {
    state: SlaveState,
    input_buffer: Buffer,
}

impl ReadData {
    fn create(address: Address) -> Slave {
        Slave::ReadData(ReadData {
            state: Box::new(SlaveStateStruct {
                slave_address: address,
                last_address: None,
                last_command: None,
            }),
            input_buffer: Buffer::new(),
        })
    }

    fn from_state(state: SlaveState) -> Slave {
        Slave::ReadData(ReadData {
            state,
            input_buffer: Buffer::new(),
        })
    }

    pub fn receive_data(mut self, data: &[u8]) -> Slave {
        self.input_buffer.write(data);

        self.parse_buffer()
    }

    fn parse_buffer(mut self) -> Slave {
        use CommandToken::*;

        let (consumed, token) = { nom_parser::parse_command(self.input_buffer.as_str_slice()) };
        self.input_buffer.consume(consumed);

        if token == NeedData {
            return Slave::ReadData(self);
        }

        // Reset is the only token we process with data remaining in the buffer
        if let Reset(address) = &token {
            self.state.last_address = match address {
                AddressToken::Valid(address) => Some(*address),
                AddressToken::Invalid => None,
            };
            self.state.clear_last_command();
        }

        // Skip this token and get another if we're not at the end of the buffer
        if self.input_buffer.len() > 0 && consumed > 0 {
            return self.parse_buffer();
        }

        match token {
            Reset(_) => {
                // see above
                Slave::ReadData(self)
            }
            ReadParameter(parameter) => ReadParam::from_state(self.state, parameter),
            WriteParameter(parameter, value) => {
                WriteParam::from_state(self.state, parameter, value)
            }
            ReadAgain(offset) => {
                if let Some(Command::Read { parameter }) = self.state.last_command {
                    if let Ok(next_param) = parameter.checked_add(offset) {
                        ReadParam::from_state(self.state, next_param)
                    } else {
                        SendData::from_state(self.state, vec![EOT])
                    }
                } else {
                    Slave::ReadData(self)
                }
            }
            SendNAK => self.send_nak(),
            NeedData => Slave::ReadData(self),
        }
    }

    fn send_nak(self) -> Slave {
        SendData::from_state(self.state, vec![NAK])
    }
}

#[derive(Debug)]
pub struct SendData {
    state: SlaveState,
    data: Vec<u8>,
}

impl SendData {
    fn from_state(state: SlaveState, data: Vec<u8>) -> Slave {
        Slave::SendData(SendData { state, data })
    }

    fn nak_from_state(state: SlaveState) -> Slave {
        Slave::SendData(SendData {
            state,
            data: vec![NAK],
        })
    }

    pub fn send_data(&mut self) -> Vec<u8> {
        self.data.split_off(0)
    }

    pub fn data_sent(self) -> Slave {
        ReadData::from_state(self.state)
    }
}

#[derive(Debug)]
pub struct ReadParam {
    state: SlaveState,
    parameter: Parameter,
}

impl ReadParam {
    fn from_state(mut state: SlaveState, parameter: Parameter) -> Slave {
        state.last_command = None;

        if state.cmd_to_slave_addr() {
            // only accept commands to our address, if we have an address
            state.last_command = Some(Command::Read { parameter });
            Slave::ReadParameter(ReadParam { state, parameter })
        } else {
            // the command was sent to another address
            ReadData::from_state(state)
        }
    }

    pub fn send_reply_ok(self, value: Value) -> Slave {
        let param = self.parameter.to_string();
        assert_eq!(param.len(), 4);
        let value = format!("{:+6}", value); //FIXME: make value length adjustable
        assert_eq!(value.len(), 6);

        let mut data = Vec::with_capacity(15);
        data.push(STX);
        data.extend_from_slice(param.as_bytes());
        data.extend_from_slice(param.as_bytes());
        data.push(ETX);
        data.push(bcc(&data[1..]));

        SendData::from_state(self.state, data)
    }

    pub fn send_invalid_parameter(self) -> Slave {
        SendData::from_state(self.state, vec![EOT])
    }

    pub fn get_address(&self) -> Address {
        self.state.last_address.unwrap()
    }
    pub fn get_parameter(&self) -> Parameter {
        self.parameter
    }
}

#[derive(Debug)]
pub struct WriteParam {
    state: SlaveState,
    parameter: Parameter,
    value: Value,
}

impl WriteParam {
    fn from_state(mut state: SlaveState, parameter: Parameter, value: Value) -> Slave {
        state.last_command = None;

        // only accept commands to our address, if we have an address
        if state.cmd_to_slave_addr() {
            state.set_last_command(Command::Write);
            Slave::WriteParameter(WriteParam {
                state,
                parameter,
                value,
            })
        } else {
            ReadData::from_state(state)
        }
    }

    pub fn get_address(&self) -> Address {
        self.state.last_address.unwrap()
    }
    pub fn get_parameter(&self) -> Parameter {
        self.parameter
    }
    pub fn get_value(&self) -> Value {
        self.value
    }

    pub fn write_ok(self) -> Slave {
        SendData::from_state(self.state, vec![ACK])
    }

    pub fn write_error(self) -> Slave {
        SendData::nak_from_state(self.state)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::slave::{Parameter, Slave, Value};
    use crate::{Address, X328Error};
    use std::cell::RefCell;
    use std::cmp::min;
    use std::collections::HashMap;
    use std::io::{Error, ErrorKind, Read, Write};
    use std::rc::Rc;

    pub(crate) struct SerialInterface {
        rx: Vec<u8>,
        rx_pos: usize,
        tx: Vec<u8>,
        do_io_error: bool,
    }

    pub(crate) struct SerialIOPlane(Rc<RefCell<SerialInterface>>);

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
                do_io_error: false,
            }))
        }

        pub fn trigger_io_error(&mut self) {
            self.do_io_error = true;
        }
    }

    impl std::io::Read for SerialIOPlane {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut inner = self.0.borrow_mut();
            if inner.do_io_error {
                inner.do_io_error = false;
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
            if inner.do_io_error {
                inner.do_io_error = false;
                Err(Error::new(ErrorKind::PermissionDenied, X328Error::IOError))
            } else {
                inner.tx.write(buf)
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn slave_main_loop() {
        let data_in = b"asd";
        let serial_sim = SerialInterface::new(data_in);
        let mut serial = SerialIOPlane::new(&serial_sim);
        let mut registers: HashMap<Parameter, Value> = HashMap::new();

        let mut slave_proto = Slave::new(Address::new(10).unwrap());

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
                    if param == 3 {
                        write_command.write_error()
                    } else {
                        registers.insert(param, write_command.get_value());
                        write_command.write_ok()
                    }
                }
            };
        }
    }
}
