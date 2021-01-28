//! See Slave for more details.

use snafu::Snafu;

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::slave::{parse_command, CommandToken};

use crate::types::{self, Address, IntoAddress, Parameter, Value};

/// Slave part of the X3.28 protocol
///
/// This enum represent the different states of the protocol.
///
/// Create a new protocol instance with Slave::new(address).
///
/// # Example
///
/// ```
/// use x328_proto::Slave;
/// # use std::io::{Read, Write, Cursor};
/// # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
/// # { Ok(Cursor::new(Vec::new())) }
/// #
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use x328_proto::Value;
/// let mut slave_proto = Slave::new(10)?; // new protocol instance with address 10
/// let mut serial = connect_serial_interface()?;
///
/// 'main: loop {
///        # break // this snippet is only for show
///        slave_proto = match slave_proto {
///            Slave::ReceiveData(recv) => {
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
///                    read_command.send_reply_ok(Value::new(4).unwrap())
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
    ReceiveData(ReceiveData),
    /// Data is waiting to be transmitted.
    SendData(SendData),
    /// A parameter read request from the bus master.
    ReadParameter(ReadParam),
    /// A parameter write request from the bus master.
    WriteParameter(WriteParam),
}

impl Slave {
    /// Create a new protocol instance, accepting commands for the given address.
    /// Returns an error if the given adress is invalid.
    /// # Example
    ///
    /// ```
    /// use x328_proto::Slave;
    /// let mut slave_proto: Slave = Slave::new(10).unwrap(); // new protocol instance with address 10
    /// ```
    pub fn new(address: impl IntoAddress) -> Result<Slave, Error> {
        Ok(ReceiveData::create(address.into_address()?))
    }

    /// Do not send any reply to the master. Transition to the idle ReceiveData state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> Slave {
        let state = match self {
            Slave::ReceiveData(ReceiveData { state, .. }) => state,
            Slave::SendData(SendData { state, .. }) => state,
            Slave::ReadParameter(ReadParam { state, .. }) => state,
            Slave::WriteParameter(WriteParam { state, .. }) => state,
        };
        ReceiveData::from_state(state)
    }
}

#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    #[snafu(display("Invalid argument {}", source), context(false))]
    InvalidArgument { source: types::Error },
}

type SlaveState = Box<SlaveStateStruct>;

#[derive(Debug)]
struct SlaveStateStruct {
    address: Address,
    read_again_param: Option<(Address, Parameter)>,
}

impl SlaveStateStruct {}

#[derive(Debug)]
pub struct ReceiveData {
    state: SlaveState,
    input_buffer: Buffer,
}

impl ReceiveData {
    fn create(address: Address) -> Slave {
        Slave::ReceiveData(ReceiveData {
            state: Box::new(SlaveStateStruct {
                address,
                read_again_param: None,
            }),
            input_buffer: Buffer::new(),
        })
    }

    fn from_state(state: SlaveState) -> Slave {
        Slave::ReceiveData(ReceiveData {
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
            match parse_command(self.input_buffer.as_ref()) {
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
            ReadParameter(address, parameter) if self.for_us(address) => {
                ReadParam::from_state(self.state, address, parameter)
            }
            WriteParameter(address, parameter, value) if self.for_us(address) => {
                WriteParam::from_state(self.state, address, parameter, value)
            }
            ReadAgain(offset) if read_again_param.is_some() => {
                if let Ok(next_param) = read_again_param.unwrap().1.checked_add(offset) {
                    ReadParam::from_state(self.state, read_again_param.unwrap().0, next_param)
                } else {
                    SendData::from_state(self.state, vec![EOT])
                }
            }
            InvalidPayload(address) if address == self.state.address => self.send_nak(),
            _ => self.need_data(), // This matches NeedData, and read/write to other addresses
        }
    }

    fn need_data(self) -> Slave {
        Slave::ReceiveData(self)
    }

    fn send_nak(self) -> Slave {
        SendData::from_state(self.state, vec![NAK])
    }

    fn for_us(&self, address: Address) -> bool {
        self.state.address == address || self.state.address == 0
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
        ReceiveData::from_state(self.state)
    }
}

#[derive(Debug)]
pub struct ReadParam {
    state: SlaveState,
    address: Address,
    parameter: Parameter,
}

impl ReadParam {
    fn from_state(state: SlaveState, address: Address, parameter: Parameter) -> Slave {
        Slave::ReadParameter(ReadParam {
            state,
            address,
            parameter,
        })
    }

    /// Send a response to the master with the value of
    /// the parameter in the read request.
    pub fn send_reply_ok(mut self, value: Value) -> Slave {
        self.state.read_again_param = Some((self.address, self.parameter));

        let mut data = Vec::with_capacity(15);
        data.push(STX);
        data.extend_from_slice(&self.parameter.to_bytes());
        data.extend_from_slice(&value.to_bytes());
        data.push(ETX);
        data.push(bcc(&data[1..]));

        SendData::from_state(self.state, data)
    }

    /// Inform the master that the parameter in the request is invalid.
    pub fn send_invalid_parameter(self) -> Slave {
        SendData::from_state(self.state, vec![EOT])
    }

    /// Do not send any reply to the master. Transition to the idle ReceiveData state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> Slave {
        ReceiveData::from_state(self.state)
    }

    /// Get the address the request was sent to.
    pub fn address(&self) -> Address {
        self.address
    }

    /// The parameter whose value is to be returned.
    pub fn parameter(&self) -> Parameter {
        self.parameter
    }
}

#[derive(Debug)]
pub struct WriteParam {
    state: SlaveState,
    address: Address,
    parameter: Parameter,
    value: Value,
}

impl WriteParam {
    fn from_state(
        state: SlaveState,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> Slave {
        Slave::WriteParameter(WriteParam {
            state,
            address,
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

    /// Do not send any reply to the master. Transition to the idle ReceiveData state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> Slave {
        ReceiveData::from_state(self.state)
    }

    /// The address the write request was sent to.
    pub fn address(&self) -> Address {
        self.address
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
