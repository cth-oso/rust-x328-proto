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

pub type OptionalAddress = Option<u8>;

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

pub enum Slave {
    ReadData(ReadData),
    SendData(SendData),
    ReadParameter(ReadParam),
    WriteParameter(WriteParam),
}

impl Slave {
    pub fn new(address: OptionalAddress) -> Slave {
        ReadData::new(address)
    }
}

#[derive(Debug)]
struct SlaveState {
    slave_address: OptionalAddress,
    last_address: OptionalAddress,
    last_command: Option<Command>,
}

impl SlaveState {
    fn set_last_command(&mut self, cmd: Command) {
        self.last_command = Some(cmd);
    }
    fn clear_last_command(&mut self) {
        self.last_command.take();
    }

    fn cmd_to_slave_addr(&self) -> bool {
        self.slave_address == self.last_address || self.slave_address.is_none()
        // NOTE: This doesn't accept the broadcast address 0
    }
}

pub struct ReadData {
    state: Option<SlaveState>,
    input_buffer: Buffer,
}

impl ReadData {
    fn new(address: Option<u8>) -> Slave {
        Slave::ReadData(ReadData {
            state: Some(SlaveState {
                slave_address: address,
                last_address: None,
                last_command: None,
            }),
            input_buffer: Buffer::new(),
        })
    }

    fn from_state(state: SlaveState) -> Slave {
        Slave::ReadData(ReadData {
            state: Some(state),
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

        // Reset is the only token we accept with data remaining in the buffer
        if let Reset(address) = &token {
            self.mut_state().last_address = match address {
                AddressToken::Valid(address) => Some(*address),
                AddressToken::Invalid => None,
            };
            self.mut_state().clear_last_command();
        }

        if self.input_buffer.len() > 0 {
            return self.parse_buffer();
        }

        match token {
            Reset(_) => {
                // see above
                Slave::ReadData(self)
            }
            ReadParameter(parameter) => ReadParam::from_state(self.take_state(), parameter),
            WriteParameter(parameter, value) => {
                WriteParam::from_state(self.take_state(), parameter, value)
            }
            ReadAgain(offset) => {
                if let Some(Command::Read { parameter }) = self.get_state().last_command {
                    let state = self.take_state();
                    if let Some(next_param) = parameter.checked_add(offset) {
                        ReadParam::from_state(state, next_param)
                    } else {
                        SendData::from_state(state, vec![EOT])
                    }
                } else {
                    Slave::ReadData(self)
                }
            }
            SendNAK => self.send_nak(),
            NeedData => Slave::ReadData(self),
        }
    }

    fn send_nak(&mut self) -> Slave {
        SendData::from_state(self.take_state(), vec![NAK])
    }

    fn get_state(&self) -> &SlaveState {
        self.state.as_ref().unwrap()
    }
    fn mut_state(&mut self) -> &mut SlaveState {
        self.state.as_mut().unwrap()
    }
    fn take_state(&mut self) -> SlaveState {
        self.state.take().unwrap()
    }
}

pub struct SendData {
    state: Option<SlaveState>,
    data: Vec<u8>,
}

impl SendData {
    fn from_state(state: SlaveState, data: Vec<u8>) -> Slave {
        Slave::SendData(SendData {
            state: Some(state),
            data,
        })
    }

    fn nak_from_state(state: SlaveState) -> Slave {
        Slave::SendData(SendData {
            state: Some(state),
            data: vec![NAK],
        })
    }

    pub fn send_data(&mut self) -> Vec<u8> {
        self.data.split_off(0)
    }

    pub fn data_sent(mut self) -> Slave {
        ReadData::from_state(self.state.take().unwrap())
    }
}

#[derive(Debug)]
pub struct ReadParam {
    state: Option<SlaveState>,
    parameter: Parameter,
}

impl ReadParam {
    fn from_state(mut state: SlaveState, parameter: Parameter) -> Slave {
        state.last_command = None;

        if state.cmd_to_slave_addr() {
            // only accept commands to our address, if we have an address
            state.last_command = Some(Command::Read { parameter });
            Slave::ReadParameter(ReadParam {
                state: Some(state),
                parameter,
            })
        } else {
            // the command was sent to another address
            ReadData::from_state(state)
        }
    }

    pub fn send_reply_ok(mut self, value: Value) -> Slave {
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

        SendData::from_state(self.take_state(), data)
    }

    pub fn send_invalid_parameter(mut self) -> Slave {
        SendData::from_state(self.take_state(), vec![EOT])
    }

    pub fn get_address(&self) -> Address {
        self.state.as_ref().unwrap().last_address.unwrap()
    }
    pub fn get_parameter(&self) -> Parameter {
        self.parameter
    }

    fn take_state(&mut self) -> SlaveState {
        self.state.take().unwrap()
    }
}

#[derive(Debug)]
pub struct WriteParam {
    state: Option<SlaveState>,
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
                state: Some(state),
                parameter,
                value,
            })
        } else {
            ReadData::from_state(state)
        }
    }

    pub fn get_address(&self) -> Address {
        self.state.as_ref().unwrap().last_address.unwrap()
    }
    pub fn get_parameter(&self) -> Parameter {
        self.parameter
    }
    pub fn get_value(&self) -> Value {
        self.value
    }

    fn take_state(&mut self) -> SlaveState {
        self.state.take().unwrap()
    }

    pub fn write_ok(mut self) -> Slave {
        SendData::from_state(self.take_state(), vec![ACK])
    }

    pub fn write_error(mut self) -> Slave {
        SendData::nak_from_state(self.take_state())
    }
}

#[cfg(test)]
mod tests {
    use crate::slave::{Parameter, Slave, Value};
    use std::cmp::min;
    use std::collections::HashMap;

    struct SerialInterface {
        rx: Vec<u8>,
        rx_pos: usize,
        tx: Vec<u8>,
    }

    impl SerialInterface {
        fn new(tx: &[u8]) -> SerialInterface {
            SerialInterface {
                tx: tx.to_vec(),
                rx: Vec::new(),
                rx_pos: 0,
            }
        }

        // Will return up to len bytes, until the rx buffer is exhausted
        fn read(&mut self, len: usize) -> Option<&[u8]> {
            let pos = self.rx_pos;
            let new_pos = min(pos + len, self.rx.len());
            if pos == new_pos {
                None
            } else {
                self.rx_pos = new_pos;
                Some(&self.rx[pos..new_pos])
            }
        }
        // Append bytes to the tx buffer
        fn write(&mut self, bytes: &[u8]) {
            self.tx.extend_from_slice(bytes);
        }
    }

    #[test]
    fn slave_main_loop() {
        let data_in = b"asd";
        let mut serial = SerialInterface::new(data_in);
        let mut registers: HashMap<Parameter, Value> = HashMap::new();

        let mut slave_proto = Slave::new(Some(10));

        'main: loop {
            slave_proto = match slave_proto {
                Slave::ReadData(recv) => {
                    if let Some(data) = serial.read(1) {
                        recv.receive_data(data)
                    } else {
                        break 'main;
                    }
                }

                Slave::SendData(mut send) => {
                    serial.write(send.send_data().as_ref());
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
