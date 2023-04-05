//! An implementation of the "node" half of the X3.28 protocol. See [`Node`] for more details.

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::node::{parse_command, CommandToken};
use crate::types::{Address, Parameter, Value};

/// Bus node (listener/server) part of the X3.28 protocol
///
/// Create a new protocol instance with `Node::new(address)`. The current protocol state can be
/// retrieved by calling `state()`. The [`NodeState`] enum returned contains structs that should
/// be acted upon in order to advance the protocol state machine.
///
/// # Example
///
/// ```
/// use x328_proto::node::{Node, NodeState, ParamRequest};
/// # use std::io::{Read, Write, Cursor};
/// # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
/// # { Ok(Cursor::new(Vec::new())) }
/// #
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use x328_proto::{addr, Value};
/// let mut node = Node::new(addr(10)); // new protocol instance with address 10
/// let mut serial = connect_serial_interface()?;
///
/// 'main: loop {
///        # break // this snippet is only for show
///        match node.state() {
///            NodeState::ReceiveData(recv) => {
///                let mut buf = [0; 1];
///                if let Ok(len) = serial.read(&mut buf) {
///                    if len == 0 {
///                        break 'main;
///                    }
///                    recv.receive_data(&buf[..len]);
///                } else {
///                    break 'main;
///                }
///            }
///
///            NodeState::SendData(mut send) => {
///                serial.write_all(send.send_data()).unwrap();
///            }
///
///            NodeState::Command(cmd) => {
///            match cmd {
///                ParamRequest::Read(read_command) => {
///                    if read_command.parameter() == 3 {
///                        read_command.send_invalid_parameter();
///                    } else {
///                        read_command.send_reply_ok(4u16.into());
///                    }
///                }
///                ParamRequest::Write(write_command) => {
///                    let param = write_command.parameter();
///                    if param == 3 {
///                        write_command.write_error();
///                    } else {
///                        write_command.write_ok();
///                    }
///                }
///            }
///        }}};
///
/// # Ok(()) }
///  ```
#[derive(Debug)]
pub struct Node {
    state: InternalState,
    address: Address,
    read_again_param: Option<(Address, Parameter)>,
    buffer: Buffer,
}

/// The current protocol state, as seen by this node.
pub enum NodeState<'node> {
    /// More data needs to be received from the bus.
    ReceiveData(ReceiveData<'node>),
    /// Data is waiting to be transmitted.
    SendData(SendData<'node>),
    /// A command to this node from the bus controller
    Command(ParamRequest<'node>),
}

/// X3.28 parameter read and write requests
#[derive(Debug)]
pub enum ParamRequest<'node> {
    /// Request for the current parameter value
    Read(ReadParam<'node>),
    /// Request to change the parameter value
    Write(WriteParam<'node>),
}

impl<'a> From<ReceiveData<'a>> for NodeState<'a> {
    fn from(x: ReceiveData<'a>) -> Self {
        Self::ReceiveData(x)
    }
}

impl<'a> From<SendData<'a>> for NodeState<'a> {
    fn from(x: SendData<'a>) -> Self {
        Self::SendData(x)
    }
}

impl<'a> From<WriteParam<'a>> for NodeState<'a> {
    fn from(x: WriteParam<'a>) -> Self {
        Self::Command(ParamRequest::Write(x))
    }
}

impl<'a> From<ReadParam<'a>> for NodeState<'a> {
    fn from(x: ReadParam<'a>) -> Self {
        Self::Command(ParamRequest::Read(x))
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum InternalState {
    Recv,
    Send,
    Read {
        address: Address,
        parameter: Parameter,
    },
    Write {
        address: Address,
        parameter: Parameter,
        value: Value,
    },
}

impl Node {
    /// Create a new protocol instance, accepting commands for the given address.
    /// # Example
    ///
    /// ```
    /// use x328_proto::{addr, node::Node};
    /// let mut node = Node::new(addr(10)); // new protocol instance with address 10
    /// ```
    pub fn new(address: Address) -> Self {
        Self {
            state: InternalState::Recv,
            address,
            read_again_param: None,
            buffer: Buffer::new(),
        }
    }

    /// Returns the current protocol state. Act on the inner structs in order to advance the
    /// protocol state machine.
    pub fn state(&mut self) -> NodeState<'_> {
        match self.state {
            InternalState::Recv => ReceiveData::from_state(self).into(),
            InternalState::Send => SendData::from_state(self).into(),
            InternalState::Read { address, parameter } => {
                ReadParam::from_state(self, address, parameter).into()
            }
            InternalState::Write {
                address,
                parameter,
                value,
            } => WriteParam::from_state(self, address, parameter, value).into(),
        }
    }

    fn set_state(&mut self, state: InternalState) {
        self.state = state;
    }

    /// Do not send any reply to the bus controller. Transition to the idle `ReceiveData` state instead.
    /// You should avoid this, since this will leave the controller waiting until it times out.
    pub fn no_reply(&mut self) -> ReceiveData {
        ReceiveData::from_state(self)
    }
}

/// "Receive data from bus" state.
#[derive(Debug)]
pub struct ReceiveData<'node> {
    node: &'node mut Node,
}

impl<'node> ReceiveData<'node> {
    fn from_state(node: &'node mut Node) -> Self {
        if node.state != InternalState::Recv {
            node.buffer.clear();
        }
        node.set_state(InternalState::Recv);
        Self { node }
    }

    /// Feed data into the internal buffer, and try to parse the buffer afterwards.
    ///
    /// A state transition will occur if a complete command has been received,
    /// or if a protocol error requires a response to be sent.
    pub fn receive_data(self, data: &[u8]) -> NodeState<'node> {
        self.node.buffer.write(data);
        self.parse_buffer()
    }

    fn parse_buffer(self) -> NodeState<'node> {
        use CommandToken::{
            InvalidPayload, ReadAgain, ReadNext, ReadParameter, ReadPrevious, WriteParameter,
        };

        let buffer = &mut self.node.buffer;

        let (token, read_again_param) = loop {
            match parse_command(buffer.as_ref()) {
                (0, _) => return self.need_data(),
                (consumed, token) => {
                    buffer.consume(consumed);
                    // Take the read again parameter from our state. It would be invalid
                    // to use it for later tokens, that's why it's extracted in the loop.
                    let read_again_param = self.node.read_again_param.take();

                    // We're done parsing when the buffer is empty
                    if buffer.len() == 0 {
                        break (token, read_again_param);
                    }
                }
            };
        };

        match token {
            ReadParameter(address, parameter) if self.for_us(address) => {
                ReadParam::from_state(self.node, address, parameter).into()
            }
            WriteParameter(address, parameter, value) if self.for_us(address) => {
                WriteParam::from_state(self.node, address, parameter, value).into()
            }
            ReadAgain | ReadNext | ReadPrevious if read_again_param.is_some() => {
                let (addr, last_param) = read_again_param.unwrap();
                match match token {
                    ReadPrevious => last_param.prev(),
                    ReadNext => last_param.next(),
                    _ => Some(last_param),
                } {
                    Some(param) => ReadParam::from_state(self.node, addr, param).into(),
                    None => SendData::from_byte(self.node, EOT).into(),
                }
            }
            InvalidPayload(address) if address == self.node.address => self.send_nak(),
            _ => self.need_data(), // This matches NeedData, and read/write to other addresses
        }
    }

    fn send_byte(self, byte: u8) -> NodeState<'node> {
        SendData::from_byte(self.node, byte).into()
    }

    fn need_data(self) -> NodeState<'node> {
        self.into()
    }

    fn send_nak(self) -> NodeState<'node> {
        self.send_byte(NAK)
    }

    fn for_us(&self, address: Address) -> bool {
        self.node.address == address || self.node.address == 0
    }
}

/// "Transmit data on the bus" state.
///
/// Call [`get_data()`](Self::get_data()) to get a reference to the data to be transmitted,
/// and then call [`data_sent()`](Self::data_sent()) when the data has been successfully transmitted.
#[derive(Debug)]
pub struct SendData<'node> {
    node: &'node mut Node,
}

impl<'node> SendData<'node> {
    /// SendData::from_state expects that the node buffer already has been prepared
    fn from_state(node: &'node mut Node) -> Self {
        node.set_state(InternalState::Send);
        Self { node }
    }

    fn from_byte(node: &'node mut Node, byte: u8) -> Self {
        let buf = &mut node.buffer;
        buf.clear();
        buf.push(byte);
        Self::from_state(node)
    }

    /// Returns the data to be sent on the bus, and changes the state to "receive data".
    pub fn send_data(self) -> &'node [u8] {
        self.node.set_state(InternalState::Recv);
        self.node.buffer.get_ref_and_clear()
    }
}

/// The "read command received" state. The bus controller expects a reply with the current
/// value of the specified parameter.
#[derive(Debug)]
pub struct ReadParam<'node> {
    node: &'node mut Node,
    address: Address,
    parameter: Parameter,
}

impl<'node> ReadParam<'node> {
    fn from_state(node: &'node mut Node, address: Address, parameter: Parameter) -> Self {
        node.set_state(InternalState::Read { address, parameter });
        Self {
            node,
            address,
            parameter,
        }
    }

    /// Send a response to the master with the value of
    /// the parameter in the read request.
    pub fn send_reply_ok(mut self, value: Value) -> SendData<'node> {
        self.node.read_again_param = Some((self.address, self.parameter));

        let data = &mut self.node.buffer;
        data.clear();

        data.push(STX);
        data.write(&self.parameter.to_bytes());
        data.write(&value.to_bytes());
        data.push(ETX);
        data.push(bcc(&data.as_ref()[1..]));

        SendData::from_state(self.node)
    }

    /// Inform the master that the parameter in the request is invalid.
    pub fn send_invalid_parameter(self) -> SendData<'node> {
        SendData::from_byte(self.node, EOT)
    }

    /// Inform the bus master that the read request failed
    /// for some reason other than invalid parameter number.
    pub fn send_read_failed(self) -> SendData<'node> {
        SendData::from_byte(self.node, NAK)
    }

    /// Do not send any reply to the master. Transition to the idle `ReceiveData` state instead.
    /// You really shouldn't do this, since this will leave the master waiting until it times out.
    pub fn no_reply(self) -> ReceiveData<'node> {
        ReceiveData::from_state(self.node)
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

/// "Write command received" state. The bus controller wants to change the value
/// of the specified parameter.
#[derive(Debug)]
pub struct WriteParam<'node> {
    node: &'node mut Node,
    address: Address,
    parameter: Parameter,
    value: Value,
}

impl<'node> WriteParam<'node> {
    fn from_state(
        node: &'node mut Node,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> Self {
        node.set_state(InternalState::Write {
            address,
            parameter,
            value,
        });
        Self {
            node,
            address,
            parameter,
            value,
        }
    }

    /// Inform the bus controller that the parameter value was successfully updated.
    pub fn write_ok(self) -> SendData<'node> {
        SendData::from_byte(self.node, ACK)
    }

    /// The parameter or value is invalid, or something else is preventing
    /// us from setting the parameter to the given value.
    pub fn write_error(self) -> SendData<'node> {
        SendData::from_byte(self.node, NAK)
    }

    /// Do not send any reply to the bus controller. Transition to the idle `ReceiveData` state instead.
    /// You should avoid this, since this will leave the controller waiting until it times out.
    pub fn no_reply(self) -> ReceiveData<'node> {
        ReceiveData::from_state(self.node)
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
