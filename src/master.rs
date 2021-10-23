//! The bus controller half of the X3.28 protocol
//!
//! # Example
//! See [`crate::master::io::Master`] for a more elaborate example of synchronus IO.
//! ```
//! # use std::io::{Read, Write, Cursor};
//! # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
//! # { Ok(Cursor::new(Vec::new())) }
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use x328_proto::{Master, IntoAddress, IntoParameter, IntoValue};
//! use x328_proto::master::{Receiver, ReceiveDataProgress};
//! let mut master = Master::new();
//! let mut serial = connect_serial_interface()?;
//!
//! let send = master.write_parameter(10.into_address()?,
//!                                   3010.into_parameter()?,
//!                                   (-30_i16).into());
//! serial.write_all(send.get_data())?;
//! let mut recv = send.data_sent();
//! loop {
//!     let mut buf = [0; 20];
//!     let len = serial.read(&mut buf[..])?;
//!     if len == 0 {
//!         // .. error handling ..
//!         # return Ok(());
//!     }
//!     match recv.receive_data(&buf[..len]) {
//!         ReceiveDataProgress::NeedData(new_recv) => recv = new_recv,
//!         ReceiveDataProgress::Done(response) => break response,
//!     }
//! }?;
//!
//! # Ok(())}
//! ```

use arrayvec::ArrayVec;
use snafu::Snafu;

use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::master::{parse_read_response, parse_write_response, ResponseToken};
use crate::types::{Address, Parameter, Value};

#[derive(Copy, Clone)]
struct NodeState {
    can_read_again: bool,
}

/// X3.28 bus controller.
pub struct Master {
    read_again: Option<(Address, Parameter)>,
    nodes: [NodeState; 100],
}

impl Debug for Master {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Master {{ read_again: {:?}, nodes: [..]}}",
            self.read_again
        )
    }
}

impl Default for Master {
    fn default() -> Self {
        Self::new()
    }
}

impl Master {
    /// Create a new instance of the X3.28 bus controller protocol.
    pub const fn new() -> Self {
        Self {
            read_again: None,
            nodes: [NodeState {
                can_read_again: false,
            }; 100],
        }
    }

    /// Initiate a write command to a node.
    ///
    /// The returned [`SendData`] struct holds the data that needs to be transmitted
    /// on the bus. It also holds a mutable reference to self, so that only one
    /// operation can be in progress at a time.
    ///
    /// Timeouts and other errors can be handled by dropping the `SendData` or
    /// `ReceiveResponse` structs.
    pub fn write_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> SendData<ReceiveWriteResponse, WriteResult> {
        self.read_again = None;
        let mut data = SendDataStore::new();
        data.push(EOT);
        data.try_extend_from_slice(&address.to_bytes())
            .expect("BUG: Send data buffer too small.");
        data.push(STX);
        data.try_extend_from_slice(&parameter.to_bytes())
            .expect("BUG: Send data buffer too small.");
        data.try_extend_from_slice(&value.to_bytes())
            .expect("BUG: Send data buffer too small.");
        data.push(ETX);
        data.push(bcc(&data[6..]));
        SendData::new(data, ReceiveWriteResponse::new(self))
    }

    /// Initiate a read command to a node.
    ///
    /// The returned [`SendData`] struct holds the data that needs to be transmitted
    /// on the bus. See also [`write_parameter()`](Self::write_parameter()).
    pub fn read_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
    ) -> SendData<ReceiveReadResponse, ReadResult> {
        let mut data = SendDataStore::new();
        if let Some(again) = self.try_read_again(address, parameter) {
            data.push(again);
        } else {
            data.push(EOT);
            data.try_extend_from_slice(&address.to_bytes()).unwrap();
            data.try_extend_from_slice(&parameter.to_bytes()).unwrap();
            data.push(ENQ);
        }
        SendData::new(data, ReceiveReadResponse::new(self, address, parameter))
    }

    /// Check if we can use the short "read-again" command form.
    /// Consumes the `self.read_again` value
    fn try_read_again(&mut self, address: Address, parameter: Parameter) -> Option<u8> {
        let (old_addr, old_param) = self.read_again.take()?;
        if old_addr == address && self.get_node_capabilities(address).can_read_again {
            match *parameter - *old_param {
                0 => Some(NAK),
                1 => Some(ACK),
                -1 => Some(BS),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Enable/disable the short form "read again" command type for a specific node.
    /// This is disabled by default for all nodes.
    pub fn read_again_enable(&mut self, address: Address, can_read_again: bool) {
        self.nodes[*address as usize] = NodeState { can_read_again };
    }

    fn get_node_capabilities(&self, address: Address) -> NodeState {
        self.nodes[*address as usize]
    }
}

type SendDataStore = ArrayVec<u8, 20>;

/// `SendData` holds data that should be transmitted to the nodes.
///
/// Call [`data_sent()`](Self::data_sent()) after the data has been
/// successfully transmitted in order to transition to the "receive
/// response" state. If data transmission fails this struct should be
/// dropped in order to return to the idle state.
#[derive(Debug)]
pub struct SendData<'a, Rec: Receiver<Res>, Res> {
    data: SendDataStore,
    receiver: Rec,
    _phantom: PhantomData<&'a Res>,
}

impl<'a, Rec: Receiver<Res>, Res> SendData<'a, Rec, Res> {
    fn new(data: SendDataStore, receiver: Rec) -> Self {
        SendData {
            data,
            receiver,
            _phantom: PhantomData::default(),
        }
    }

    /// Returns a reference to the data to be transmitted.
    pub fn get_data(&self) -> &[u8] {
        self.data.as_slice()
    }

    /// Call after data has been successfully transmitted in order
    /// to transition to the "receive response" state.
    pub fn data_sent(self) -> Rec {
        self.receiver
    }
}

mod private {
    pub trait Receiver {}
}

/// Return value from `Receiver::receive_data()`
/// Indicates if enough data has been received or if more data is needed.
/// R is the receiver (Self), T is `Self::Response`
pub enum ReceiveDataProgress<R, T> {
    /// A complete response has been received, `T` is the result.
    Done(T),
    /// More data is needed. Read for the bus and pass the data to `R`.
    NeedData(R),
}

/// Provides the `receive_data()` method for parsing response
/// data from the nodes.
pub trait Receiver<Response>: Sized + private::Receiver {
    /// Receive and parse data from the bus.
    ///
    /// Note that the method consumes self, so it must be reclaimed
    /// from the return value.
    fn receive_data(self, data: &[u8]) -> ReceiveDataProgress<Self, Response>;
}

type WriteResult = Result<(), Error>;

/// Call [`receive_data()`](Receiver::receive_data()) to process the
/// received response data from the node.
///
/// Test that the borrow-checker prevents concurrent commands
/// ```compile_fail
/// use x328_proto::Master;
/// use x328_proto::master::Receiver;
/// use x328_proto::types::{addr, param, value};
/// let mut m = Master::new();
/// let s = m.write_parameter(addr(10), param(20), value(30));
/// let mut r = s.data_sent();
/// let s = m.write_parameter(addr(12), param(11), value(0));
/// r.receive_data(&[1]);
/// ```
#[derive(Debug)]
pub struct ReceiveWriteResponse<'a> {
    _master: PhantomData<&'a mut Master>,
    buffer: Buffer,
}

impl<'a> ReceiveWriteResponse<'a> {
    fn new(_master: &'a mut Master) -> Self {
        ReceiveWriteResponse {
            _master: PhantomData,
            buffer: Buffer::new(),
        }
    }
}

impl private::Receiver for ReceiveWriteResponse<'_> {}

impl Receiver<WriteResult> for ReceiveWriteResponse<'_> {
    fn receive_data(mut self, data: &[u8]) -> ReceiveDataProgress<Self, WriteResult> {
        self.buffer.write(data);

        ReceiveDataProgress::Done(match parse_write_response(self.buffer.as_ref()) {
            ResponseToken::WriteOk => Ok(()),
            ResponseToken::WriteFailed | ResponseToken::InvalidParameter => CommandFailed.fail(),
            _ => ProtocolError.fail(),
        })
    }
}

type ReadResult = Result<Value, Error>;

/// Error type for the X3.28 bus controller
#[derive(Debug, Snafu)]
pub enum Error {
    /// The node responded `EOT` to a command, indicating that
    /// the sent `Parameter` doesn't exist on the node.
    #[snafu(display("Node responded EOT to command."))]
    InvalidParameter,
    /// `NAK` response from node, indicating that the command
    /// couldn't be processed successfully.
    #[snafu(display("Node responded NAK to command."))]
    CommandFailed,
    /// Invalid data received from node, or some other protocol
    /// failure.
    #[snafu(display("Invalid response from node."))]
    ProtocolError,
}

/// This struct implements the `Receiver` trait to receive and process
/// the response to a read command.
#[derive(Debug)]
pub struct ReceiveReadResponse<'a> {
    master: &'a mut Master,
    buffer: Buffer,
    address: Address,
    expected_param: Parameter,
}

impl<'a> ReceiveReadResponse<'a> {
    fn new(
        master: &'a mut Master,
        address: Address,
        parameter: Parameter,
    ) -> ReceiveReadResponse<'a> {
        ReceiveReadResponse {
            master,
            buffer: Buffer::new(),
            address,
            expected_param: parameter,
        }
    }
}

impl private::Receiver for ReceiveReadResponse<'_> {}

impl Receiver<ReadResult> for ReceiveReadResponse<'_> {
    fn receive_data(mut self, data: &[u8]) -> ReceiveDataProgress<Self, ReadResult> {
        self.buffer.write(data);

        ReceiveDataProgress::Done(match parse_read_response(self.buffer.as_ref()) {
            ResponseToken::NeedData => return ReceiveDataProgress::NeedData(self),
            ResponseToken::ReadOk { parameter, value } if (parameter == self.expected_param) => {
                self.master.read_again = Some((self.address, parameter));
                ReadResult::Ok(value)
            }
            ResponseToken::InvalidParameter => InvalidParameter {}.fail(),
            _ => ProtocolError.fail(),
        })
    }
}

/// Sample implementation of the X3.28 bus controller
/// for an IO-channel implementing `std::io::{Read, Write}`.
pub mod io {
    use snafu::{ResultExt, Snafu};

    use crate::master::{Error as X328Error, ReceiveDataProgress, Receiver, SendData};
    use crate::types::{self, IntoAddress, IntoParameter, IntoValue, Value};
    use crate::{Address, Parameter};
    use std::io::{Read, Write};

    /// Error type for `master::io`.
    #[derive(Debug, Snafu)]
    pub enum Error {
        /// Conversion of a given argument to `Address`, `Parameter`
        /// or `Value` failed.
        #[snafu(display("Invalid argument"))]
        InvalidArgument {
            /// The type of arg that failed conversion.
            source: types::Error,
        },
        /// Errors generated by the X3.28 protocol
        #[snafu(display("X3.28 command error"))]
        ProtocolError {
            /// The original X3.28 error.
            source: X328Error,
        },
        /// Errors from std::io
        #[snafu(display("X3.28 IO error: {}", source))]
        IoError {
            /// The original std::io error
            source: std::io::Error,
        },
    }

    trait ReceiveFrom<Res>: Receiver<Res> {
        fn receive_from(self, reader: &mut impl Read) -> Result<Res, Error>;
    }

    impl<R: Receiver<Res>, Res> ReceiveFrom<Res> for R {
        fn receive_from(mut self, reader: &mut impl Read) -> Result<Res, Error> {
            let mut data = [0];
            loop {
                let len = match reader.read(&mut data) {
                    Ok(0) => Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Read returned Ok(0)",
                    )),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    x => x,
                }
                .context(IoError {})?;

                match self.receive_data(&data[..len]) {
                    ReceiveDataProgress::Done(response) => return Ok(response),
                    ReceiveDataProgress::NeedData(reader) => self = reader,
                }
            }
        }
    }

    trait WriteData<R> {
        fn write_to(self, writer: &mut impl std::io::Write) -> Result<R, Error>;
    }

    impl<Rec, Res> WriteData<Rec> for SendData<'_, Rec, Res>
    where
        Rec: Receiver<Res>,
    {
        fn write_to(self, writer: &mut impl Write) -> Result<Rec, Error> {
            match writer
                .write_all(self.get_data())
                .and_then(|_| writer.flush())
            {
                Ok(_) => Ok(self.data_sent()),
                Err(err) => Err(err),
            }
            .context(IoError {})
        }
    }

    /// X3.28 bus controller with IO using the `std::io::{Read, Write}` traits.
    #[derive(Debug)]
    pub struct Master<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        proto: super::Master,
        stream: IO,
    }

    impl<IO> Master<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        /// Create a new protocol instance, with `io` as transport.
        pub fn new(io: IO) -> Self {
            Self {
                proto: super::Master::new(),
                stream: io,
            }
        }

        /// Enable/disable the short form "read again" command for a
        /// specific node address.
        pub fn set_can_read_again(&mut self, address: Address, value: bool) {
            self.proto.read_again_enable(address, value);
        }

        /// Send a write command to the node.
        pub fn write_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
            value: impl IntoValue,
        ) -> Result<(), Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let value = value.into_value().context(InvalidArgument {})?;
            let response = self
                .proto
                .write_parameter(address, parameter, value)
                .write_to(&mut self.stream)?
                .receive_from(&mut self.stream)?;
            response.context(ProtocolError)
        }

        /// Send a read command to the node
        pub fn read_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
        ) -> Result<Value, Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let response = self
                .proto
                .read_parameter(address, parameter)
                .write_to(&mut self.stream)?
                .receive_from(&mut self.stream)?;
            response.context(ProtocolError)
        }
    } // impl Master

    fn check_addr_param(
        addr: impl IntoAddress,
        param: impl IntoParameter,
    ) -> Result<(Address, Parameter), Error> {
        Ok((
            addr.into_address().context(InvalidArgument {})?,
            param.into_parameter().context(InvalidArgument {})?,
        ))
    }
} // mod io

/// Tests for the base sans-IO master implementation
#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryInto;

    fn addr_param_val(addr: usize, param: usize, val: i32) -> (Address, Parameter, Value) {
        (
            addr.try_into().unwrap(),
            param.try_into().unwrap(),
            val.try_into().unwrap(),
        )
    }

    #[test]
    fn write_parameter() {
        let (addr, param, val) = addr_param_val(43, 1234, 56);
        let mut master = Master::new();
        let x = master.write_parameter(addr, param, val);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.get_data(), b"\x044433\x021234+56\x03\x2F");
    }

    #[test]
    fn read_parameter() {
        let (addr, param, _) = addr_param_val(43, 1234, 56);
        let mut master = Master::new();
        let x = master.read_parameter(addr, param);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.get_data(), b"\x0444331234\x05");
    }

    #[test]
    fn read_again() {
        let (addr, param, _) = addr_param_val(10, 20, 56);
        let mut idle = Master::new();
        idle.read_again_enable(addr, true);
        idle.read_again = Some((addr, param));
        let send = idle.read_parameter(addr, param.next().unwrap());
        assert_eq!(send.get_data(), [ACK]);
    }
}
