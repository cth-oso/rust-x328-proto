//! See [`BusNode`] for more details.

use arrayvec::ArrayVec;

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::node::{parse_command, CommandToken};
use crate::types::{Address, Error as TypeError, IntoAddress, Parameter, Value};

/// Bus node (listener/server) part of the X3.28 protocol
///
/// This enum represents the different states of the protocol.
///
/// Create a new protocol instance with `Node::new(address)`.
///
/// # Example
///
/// ```
/// use x328_proto::BusNode;
/// # use std::io::{Read, Write, Cursor};
/// # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
/// # { Ok(Cursor::new(Vec::new())) }
/// #
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use x328_proto::Value;
/// let mut node = BusNode::new(10)?; // new protocol instance with address 10
/// let mut serial = connect_serial_interface()?;
///
/// 'main: loop {
///        # break // this snippet is only for show
///        node = match node {
///            BusNode::ReceiveData(recv) => {
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
///            BusNode::SendData(mut send) => {
///                serial.write_all(send.get_data()).unwrap();
///                send.data_sent()
///            }
///
///            BusNode::ReadParameter(read_command) => {
///                if read_command.parameter() == 3 {
///                    read_command.send_invalid_parameter()
///                } else {
///                    read_command.send_reply_ok(4u16.into())
///                }
///            }
///
///            BusNode::WriteParameter(write_command) => {
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
pub enum BusNode {
    /// More data needs to be received from the bus. Use receive_data() on the inner struct.
    ReceiveData(ReceiveData),
    /// Data is waiting to be transmitted.
    SendData(SendData),
    /// A parameter read request from the bus master.
    ReadParameter(ReadParam),
    /// A parameter write request from the bus master.
    WriteParameter(WriteParam),
}

impl BusNode {
    /// Create a new protocol instance, accepting commands for the given address.
    /// Returns an error if the given adress is invalid.
    /// # Example
    ///
    /// ```
    /// use x328_proto::BusNode;
    /// let mut node: BusNode = BusNode::new(10).unwrap(); // new protocol instance with address 10
    /// ```
    pub fn new(address: impl IntoAddress) -> Result<Self, TypeError> {
        Ok(ReceiveData::create(address.into_address()?))
    }

    /// Do not send any reply to the master. Transition to the idle `ReceiveData` state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> Self {
        match self {
            Self::ReceiveData(ReceiveData { state, .. })
            | Self::SendData(SendData { state, .. })
            | Self::ReadParameter(ReadParam { state, .. })
            | Self::WriteParameter(WriteParam { state, .. }) => ReceiveData::from_state(state),
        }
    }
}

type NodeState = Box<NodeStateStruct>;

#[derive(Debug)]
struct NodeStateStruct {
    address: Address,
    read_again_param: Option<(Address, Parameter)>,
}

impl NodeStateStruct {}

/// Struct with methods for the "receive data from bus" state.
#[derive(Debug)]
pub struct ReceiveData {
    state: NodeState,
    input_buffer: Buffer,
}

impl ReceiveData {
    fn create(address: Address) -> BusNode {
        BusNode::ReceiveData(Self {
            state: Box::new(NodeStateStruct {
                address,
                read_again_param: None,
            }),
            input_buffer: Buffer::new(),
        })
    }

    fn from_state(state: NodeState) -> BusNode {
        BusNode::ReceiveData(Self {
            state,
            input_buffer: Buffer::new(),
        })
    }

    /// Feed data into the internal buffer, and try to parse the buffer afterwards.
    ///
    /// A state transition will occur if a complete command has been received,
    /// or if a protocol error requires a response to be sent.
    pub fn receive_data(mut self, data: &[u8]) -> BusNode {
        self.input_buffer.write(data);

        self.parse_buffer()
    }

    fn parse_buffer(mut self) -> BusNode {
        use CommandToken::{InvalidPayload, ReadAgain, ReadParameter, WriteParameter};

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
                    SendData::from_byte(self.state, EOT)
                }
            }
            InvalidPayload(address) if address == self.state.address => self.send_nak(),
            _ => self.need_data(), // This matches NeedData, and read/write to other addresses
        }
    }

    const fn need_data(self) -> BusNode {
        BusNode::ReceiveData(self)
    }

    fn send_nak(self) -> BusNode {
        SendData::from_byte(self.state, NAK)
    }

    fn for_us(&self, address: Address) -> bool {
        self.state.address == address || self.state.address == 0
    }
}

// length: STX<param (4)><value (6)>ETX<bcc> == 13
type SendDataStore = ArrayVec<u8, 13>;

/// Struct with methods for the "transmit data on bus" state.
///
/// Call [`send_data()`](Self::send_data()) to get a reference to the data to be transmitted,
/// and then call [`data_sent()`](Self::data_sent()) when the data has been successfully transmitted.
#[derive(Debug)]
pub struct SendData {
    state: NodeState,
    data: SendDataStore,
}

impl SendData {
    fn from_state(state: NodeState, data: SendDataStore) -> BusNode {
        BusNode::SendData(Self { state, data })
    }

    fn from_byte(state: NodeState, byte: u8) -> BusNode {
        let mut data = ArrayVec::new();
        data.push(byte);
        BusNode::SendData(Self { state, data })
    }

    /// Returns the data to be sent on the bus.
    pub fn get_data(&mut self) -> &[u8] {
        self.data.as_ref()
    }

    /// Signals that the data was sent, and it's time to go back to the
    /// `ReadData` state.
    pub fn data_sent(self) -> BusNode {
        ReceiveData::from_state(self.state)
    }
}

/// Struct representing the "read command received" state.
#[derive(Debug)]
pub struct ReadParam {
    state: NodeState,
    address: Address,
    parameter: Parameter,
}

impl ReadParam {
    fn from_state(state: NodeState, address: Address, parameter: Parameter) -> BusNode {
        BusNode::ReadParameter(Self {
            state,
            address,
            parameter,
        })
    }

    /// Send a response to the master with the value of
    /// the parameter in the read request.
    pub fn send_reply_ok(mut self, value: Value) -> BusNode {
        self.state.read_again_param = Some((self.address, self.parameter));

        let mut data = SendDataStore::new();
        data.push(STX);
        data.try_extend_from_slice(&self.parameter.to_bytes())
            .expect("BUG: Send buffer too small.");
        data.try_extend_from_slice(&value.to_bytes())
            .expect("BUG: Send buffer too small.");
        data.push(ETX);
        data.push(bcc(&data[1..]));

        SendData::from_state(self.state, data)
    }

    /// Inform the master that the parameter in the request is invalid.
    pub fn send_invalid_parameter(self) -> BusNode {
        SendData::from_byte(self.state, EOT)
    }

    /// Inform the bus master that the read request failed
    /// for some reason other than invalid parameter number.
    pub fn send_read_failed(self) -> BusNode {
        SendData::from_byte(self.state, NAK)
    }

    /// Do not send any reply to the master. Transition to the idle `ReceiveData` state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> BusNode {
        ReceiveData::from_state(self.state)
    }

    /// Get the address the request was sent to.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// The parameter whose value is to be returned.
    pub const fn parameter(&self) -> Parameter {
        self.parameter
    }
}

/// Struct representing the "write command received" state.
#[derive(Debug)]
pub struct WriteParam {
    state: NodeState,
    address: Address,
    parameter: Parameter,
    value: Value,
}

impl WriteParam {
    fn from_state(
        state: NodeState,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> BusNode {
        BusNode::WriteParameter(Self {
            state,
            address,
            parameter,
            value,
        })
    }

    /// Inform the master that the parameter value was successfully updated.
    pub fn write_ok(self) -> BusNode {
        SendData::from_byte(self.state, ACK)
    }

    /// The parameter or value is invalid, or something else is preventing
    /// us from setting the parameter to the given value.
    pub fn write_error(self) -> BusNode {
        SendData::from_byte(self.state, NAK)
    }

    /// Do not send any reply to the master. Transition to the idle `ReceiveData` state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> BusNode {
        ReceiveData::from_state(self.state)
    }

    /// The address the write request was sent to.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// The parameter to be written.
    pub const fn parameter(&self) -> Parameter {
        self.parameter
    }

    /// The new value for the parameter.
    pub const fn value(&self) -> Value {
        self.value
    }
}
