//! See Slave for more details.

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

use crate::{Address, Parameter, Value};

/// Slave part of the X3.28 protocol
///
/// This enum represent the different states of the protocol.
///
/// Create a new protocol instance with Slave::new(address).
///
/// # Example
///
/// ```
/// use x328_proto::{Address, Slave};
/// # use std::io::{Read, Write, Cursor};
/// # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
/// # { Ok(Cursor::new(Vec::new())) }
/// #
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let my_address = Address::new(10)?;
/// let mut slave_proto = Slave::new(my_address); // new protocol instance with address 10
/// let mut serial = connect_serial_interface()?;
///
/// 'main: loop {
///        # break // this snippet is only for show
///        slave_proto = match slave_proto {
///            Slave::ReadData(recv) => {
///                let mut buf = [0; 1];
///                if let Ok(len) = serial.read(&mut buf) {
///                    if len == 0 {
///                        break 'main;
///                    }
///                    recv.receive_data(&buf[..len])
///                } else {
///                    break 'main;
///                }
///            }
///
///            Slave::SendData(mut send) => {
///                serial.write_all(send.send_data().as_ref()).unwrap();
///                send.data_sent()
///            }
///
///            Slave::ReadParameter(read_command) => {
///                if read_command.parameter() == 3 {
///                    read_command.send_invalid_parameter()
///                } else {
///                    read_command.send_reply_ok(4)
///                }
///            }
///
///            Slave::WriteParameter(write_command) => {
///                let param = write_command.parameter();
///                if param == 3 {
///                    write_command.write_error()
///                } else {
///                    write_command.write_ok()
///                }
///            }
///        };
/// }
/// # Ok(()) }
///  ```
#[derive(Debug)]
pub enum Slave {
    /// More data needs to be received from the bus. Use receive_data() on the inner struct.
    ReadData(ReadData),
    /// Data is waiting to be transmitted.
    SendData(SendData),
    /// A parameter read request from the bus master.
    ReadParameter(ReadParam),
    /// A parameter write request from the bus master.
    WriteParameter(WriteParam),
}

impl Slave {
    /// Create a new protocol instance, accepting commands for the given address.
    /// # Example
    ///
    /// ```
    /// use x328_proto::{Address, Slave};
    /// let my_address = Address::new(10).unwrap();
    /// let mut slave_proto = Slave::new(my_address); // new protocol instance with address 10
    /// ```
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

    /// Feed data into the internal buffer, and try to parse the buffer afterwards.
    ///
    /// A state transition will occur if a complete command has been received,
    /// or if a protocol error requires a response to be sent.
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

    /// Returns the data to be sent on the bus. Subsequent calls will return
    /// an empty Vec<u8>.
    pub fn send_data(&mut self) -> Vec<u8> {
        self.data.split_off(0)
    }

    /// Signals that the data was sent, and it's time to go back to the
    /// ReadData state.
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

    /// Send a response to the master with the value of
    /// the parameter in the read request.
    pub fn send_reply_ok(mut self, value: Value) -> Slave {
        self.state.read_again_param = Some(self.parameter);

        let value = format!("{:+06}", value);
        assert_eq!(value.len(), 6);

        let mut data = Vec::with_capacity(15);
        data.push(STX);
        data.extend_from_slice(&self.parameter.to_bytes());
        data.extend_from_slice(value.as_bytes());
        data.push(ETX);
        data.push(bcc(&data[1..]));

        SendData::from_state(self.state, data)
    }

    /// Inform the master that the parameter in the request is invalid.
    pub fn send_invalid_parameter(self) -> Slave {
        SendData::from_state(self.state, vec![EOT])
    }

    /// Get the address the request was sent to. This is always our address,
    /// bar programming error.
    pub fn address(&self) -> Address {
        self.state.address
    }

    /// The parameter whose value is to be returned.
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

    /// Inform the master that the parameter value was successfully updated.
    pub fn write_ok(self) -> Slave {
        SendData::from_state(self.state, vec![ACK])
    }

    /// The parameter or value is invalid, or something else is preventing
    /// us from setting the parameter to the given value.
    pub fn write_error(self) -> Slave {
        SendData::nak_from_state(self.state)
    }

    /// The address the write request was sent to. This is always our address.
    pub fn address(&self) -> Address {
        self.state.address
    }

    /// The parameter to be written.
    pub fn parameter(&self) -> Parameter {
        self.parameter
    }

    /// The new value for the parameter.
    pub fn value(&self) -> Value {
        self.value
    }
}
