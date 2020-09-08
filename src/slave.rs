use ascii::AsciiChar;

const ACK: u8 = AsciiChar::ACK.as_byte();
// const BS: u8 = AsciiChar::BackSpace.as_byte();
// const ENQ: u8 = AsciiChar::ENQ.as_byte();
const EOT: u8 = AsciiChar::EOT.as_byte();
const ETX: u8 = AsciiChar::ETX.as_byte();
const NAK: u8 = AsciiChar::NAK.as_byte();
const STX: u8 = AsciiChar::SOX.as_byte();

use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::slave::{parse_command, CommandToken};

pub use crate::{Address, Parameter, Value};

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
    address: Address,
    read_again_param: Option<Parameter>,
}

impl SlaveStateStruct {}

#[derive(Debug)]
pub struct ReadData {
    state: SlaveState,
    input_buffer: Buffer,
}

impl ReadData {
    fn create(address: Address) -> Slave {
        Slave::ReadData(ReadData {
            state: Box::new(SlaveStateStruct {
                address,
                read_again_param: None,
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

        let (token, read_again_param) = loop {
            match parse_command(self.input_buffer.as_str_slice()) {
                (0, _) => return self.need_data(),
                (consumed, token) => {
                    self.input_buffer.consume(consumed);
                    // Take the read again parameter from our state. It would be invalid
                    // to use it for later tokens, that's why it's extracted in the loop.
                    let read_again_param = self.state.read_again_param.take();

                    // We're done parsing when the buffer is empty
                    if self.input_buffer.len() == 0 {
                        break (token, read_again_param);
                    }
                }
            };
        };

        match token {
            ReadParameter(address, parameter) if address == self.state.address => {
                ReadParam::from_state(self.state, parameter)
            }
            WriteParameter(address, parameter, value) if address == self.state.address => {
                WriteParam::from_state(self.state, parameter, value)
            }
            ReadAgain(offset) if read_again_param.is_some() => {
                if let Ok(next_param) = read_again_param.unwrap().checked_add(offset) {
                    ReadParam::from_state(self.state, next_param)
                } else {
                    SendData::from_state(self.state, vec![EOT])
                }
            }
            InvalidPayload(address) if address == self.state.address => self.send_nak(),
            _ => self.need_data(), // This matches NeedData, and read/write to other addresses
        }
    }

    fn need_data(self) -> Slave {
        Slave::ReadData(self)
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
    fn from_state(state: SlaveState, parameter: Parameter) -> Slave {
        Slave::ReadParameter(ReadParam { state, parameter })
    }

    pub fn send_reply_ok(mut self, value: Value) -> Slave {
        self.state.read_again_param = Some(self.parameter);
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

    pub fn address(&self) -> Address {
        self.state.address
    }
    pub fn parameter(&self) -> Parameter {
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
    fn from_state(state: SlaveState, parameter: Parameter, value: Value) -> Slave {
        Slave::WriteParameter(WriteParam {
            state,
            parameter,
            value,
        })
    }

    pub fn address(&self) -> Address {
        self.state.address
    }
    pub fn parameter(&self) -> Parameter {
        self.parameter
    }
    pub fn value(&self) -> Value {
        self.value
    }

    pub fn write_ok(self) -> Slave {
        SendData::from_state(self.state, vec![ACK])
    }

    pub fn write_error(self) -> Slave {
        SendData::nak_from_state(self.state)
    }
}
