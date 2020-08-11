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

        if token == NeedData {
            return self.need_data();
        }
        self.input_buffer.consume(consumed);

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
                self.need_data()
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
                    self.need_data()
                }
            }
            SendNAK => self.send_nak(),
            NeedData => self.need_data(),
        }
    }

    fn need_data(self) -> Slave {
        Slave::ReadData(self)
    }

    fn send_nak(mut self) -> Slave {
        self.state.last_address = None;
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
        let value = format!("{:+06}", value);
        assert_eq!(value.len(), 6);

        let mut data = Vec::with_capacity(15);
        data.push(STX);
        data.extend_from_slice(param.as_bytes());
        data.extend_from_slice(value.as_bytes());
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
