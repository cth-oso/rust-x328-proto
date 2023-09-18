//! The bus controller half of the X3.28 protocol
//!
//! # Example
//! See [`crate::master::io::Master`] for a more elaborate example of synchronous IO.
//! ```
//! # use std::io::{Read, Write, Cursor};
//! # fn connect_serial_interface() -> Result<Cursor<Vec<u8>>,  &'static str>
//! # { Ok(Cursor::new(Vec::new())) }
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use x328_proto::{Master, addr, param, master::SendData};
//! let mut master = Master::new();
//! let mut serial = connect_serial_interface()?;
//!
//! let send = &mut master.write_parameter(addr(10), param(3010), (-30_i16).into());
//! serial.write_all(send.get_data())?;
//! let mut recv = send.data_sent();
//! loop {
//!     let mut buf = [0; 20];
//!     let len = serial.read(&mut buf[..])?;
//!     if len == 0 {
//!         // .. error handling ..
//!         # return Ok(());
//!     }
//!     if let Some(response) = recv.receive_data(&buf[..len]) {
//!         break response;
//!     }
//! }?;
//!
//! # Ok(())}
//! ```

use snafu::Snafu;

use core::fmt::{self, Debug, Formatter};

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::master::{parse_read_response, parse_write_response, ResponseToken};
use crate::types::{Address, Parameter, Value};

/// X3.28 bus controller.
pub struct Master {
    read_again: Option<(Address, Parameter)>,
}

impl Debug for Master {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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
        Self { read_again: None }
    }

    /// Initiate a write command to a node.
    ///
    /// The returned opaque type holds the data that should be transmitted
    /// on the bus. It also holds a mutable reference to self, so that only one
    /// operation can be in progress at a time.
    ///
    /// Timeouts and other errors can be handled by dropping the returned value.
    pub fn write_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> impl SendData<Response = ()> + '_ {
        self.read_again = None;
        let mut data = Buffer::new();
        data.push(EOT);
        data.write(&address.to_bytes());
        data.push(STX);
        data.write(&parameter.to_bytes());
        data.write(&value.to_bytes());
        data.push(ETX);
        data.push(bcc(&data.as_ref()[6..]));
        WriteCmd { data }
    }

    /// Initiate a read command to a node.
    ///
    /// The returned opaque type holds the data that should be transmitted
    /// on the bus. See also [`write_parameter()`](Self::write_parameter()).
    pub fn read_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
    ) -> impl SendData<Response = Value> + '_ {
        let mut buffer = Buffer::new();
        self.read_again.take(); // clear the "read again" state
        buffer.push(EOT);
        buffer.write(&address.to_bytes());
        buffer.write(&parameter.to_bytes());
        buffer.push(ENQ);

        ReadCmd {
            master: self,
            buffer,
            parameter,
            read_again: None,
        }
    }

    /// Initiate a read command to a node. This method may use the abbreviated command form
    /// for consecutive reads from a node.
    pub fn read_parameter_again(
        &mut self,
        address: Address,
        parameter: Parameter,
    ) -> impl SendData<Response = Value> + '_ {
        let mut buffer = Buffer::new();
        if let Some(again) = self.try_read_again(address, parameter) {
            buffer.push(again);
        } else {
            buffer.push(EOT);
            buffer.write(&address.to_bytes());
            buffer.write(&parameter.to_bytes());
            buffer.push(ENQ);
        }

        ReadCmd {
            master: self,
            buffer,
            parameter,
            read_again: Some(address),
        }
    }

    /// Check if we can use the short "read-again" command form.
    /// Consumes the `self.read_again` value
    fn try_read_again(&mut self, address: Address, parameter: Parameter) -> Option<u8> {
        let (old_addr, old_param) = self.read_again.take()?;
        if old_addr == address {
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
}

/// `SendData` holds data that should be transmitted to the nodes.
///
/// Call [`data_sent()`](Self::data_sent()) after the data has been
/// successfully transmitted in order to transition to the "receive
/// response" state. If data transmission fails this struct should be
/// dropped in order to return to the idle state.
///
pub trait SendData {
    /// The type of the value of the response to the query
    type Response;
    /// Returns the data that is to be sent on the bus to the nodes.
    fn get_data(&self) -> &[u8];
    /// Call when the data has been sent successfully and it is time to receive the response.
    fn data_sent(&mut self) -> &mut dyn ReceiveData<Response = Self::Response>;
}

/// Receives the command response from the node. Keep reading data from the bus
/// until receive_data() returns Some(..).
pub trait ReceiveData {
    /// The type of the value of the response to the query
    type Response;
    /// Parse the query response from the nodes. Keep reading from the bus until Some(..) is returned.
    fn receive_data(&mut self, data: &[u8]) -> Option<Result<Self::Response, Error>>;
}

const WRITE_BUF_LEN: usize = 1 + 4 + 1 + 4 + 6 + 1 + 1; // EOT addr STX param value ETX bcc
struct WriteCmd {
    data: Buffer<WRITE_BUF_LEN>,
}

impl SendData for WriteCmd {
    type Response = ();

    fn get_data(&self) -> &[u8] {
        self.data.as_ref()
    }

    fn data_sent(&mut self) -> &mut dyn ReceiveData<Response = Self::Response> {
        self.data.clear();
        self
    }
}

impl ReceiveData for WriteCmd {
    type Response = ();

    fn receive_data(&mut self, data: &[u8]) -> Option<Result<Self::Response, Error>> {
        Some(match parse_write_response(data) {
            ResponseToken::WriteOk => Ok(()),
            // FIXME: restructure errors
            ResponseToken::WriteFailed | ResponseToken::InvalidParameter => {
                CommandFailedSnafu.fail()
            }
            _ => ProtocolSnafu.fail(),
        })
    }
}

const READ_CMD_BUF_LEN: usize = 1 + 4 + 6 + 1 + 1; // the response must fit in this buffer
struct ReadCmd<'a> {
    master: &'a mut Master,
    buffer: Buffer<READ_CMD_BUF_LEN>,
    parameter: Parameter,
    read_again: Option<Address>,
}

impl SendData for ReadCmd<'_> {
    type Response = Value;

    fn get_data(&self) -> &[u8] {
        self.buffer.as_ref()
    }

    fn data_sent(&mut self) -> &mut dyn ReceiveData<Response = Self::Response> {
        self.buffer.clear();
        self
    }
}

impl ReceiveData for ReadCmd<'_> {
    type Response = Value;

    fn receive_data(&mut self, data: &[u8]) -> Option<Result<Self::Response, Error>> {
        self.buffer.write(data);

        Some(match parse_read_response(self.buffer.as_ref()) {
            ResponseToken::NeedData => return None,
            ResponseToken::ReadOk { parameter, value } if (parameter == self.parameter) => {
                self.master.read_again = self.read_again.map(|addr| (addr, self.parameter));
                Ok(value)
            }
            ResponseToken::InvalidParameter => InvalidParameterSnafu.fail(),
            _ => ProtocolSnafu.fail(),
        })
    }
}

/// Error type for the X3.28 bus controller
#[derive(Debug, Clone, Snafu)]
pub enum Error {
    /// The node responded `EOT` to a command, indicating that
    /// the sent `Parameter` doesn't exist on the node.
    #[snafu(display("Invalid parameter, EOT received."))]
    InvalidParameter,
    /// `NAK` response from node, indicating that the command
    /// couldn't be processed successfully.
    #[snafu(display("Command failed, NAK received."))]
    CommandFailed,
    /// Invalid data received from node, or some other protocol
    /// failure.
    #[snafu(display("Invalid response from node."))]
    ProtocolError,
}

#[cfg(any(feature = "std", test))]
/// Sample implementation of the X3.28 bus controller
/// for an IO-channel implementing `std::io::{Read, Write}`.
pub mod io {
    use snafu::{ResultExt, Snafu};

    use crate::master::{Error as X328Error, ReceiveData, SendData};
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

        /// Send a write command to the node.
        pub fn write_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
            value: impl IntoValue,
        ) -> Result<(), Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let value = value.into_value().context(InvalidArgumentSnafu)?;
            let s = self.proto.write_parameter(address, parameter, value);
            Self::send_recv(s, &mut self.stream)
        }

        /// Send a read command to the node
        pub fn read_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
        ) -> Result<Value, Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let s = self.proto.read_parameter(address, parameter);
            Self::send_recv(s, &mut self.stream)
        }

        /// Read node register using the abbreviated command form for consecutive reads.
        pub fn read_parameter_again(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
        ) -> Result<Value, Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let s = self.proto.read_parameter_again(address, parameter);
            Self::send_recv(s, &mut self.stream)
        }

        fn send_recv<R>(
            mut send: impl SendData<Response = R>,
            mut io: impl Read + Write,
        ) -> Result<R, Error> {
            let r = Self::send_data(&mut send, &mut io)?;
            Self::recv_response(r, io)
        }

        fn send_data<R>(
            send: &mut dyn SendData<Response = R>,
            mut writer: impl Write,
        ) -> Result<&mut dyn ReceiveData<Response = R>, Error> {
            log::trace!("Sending {:?}", send.get_data());
            match writer
                .write_all(send.get_data())
                .and_then(|_| writer.flush())
            {
                Ok(_) => Ok(send.data_sent()),
                Err(err) => Err(err),
            }
            .context(IoSnafu {})
        }

        fn recv_response<R>(
            recv: &mut dyn ReceiveData<Response = R>,
            mut reader: impl Read,
        ) -> Result<R, Error> {
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
                .context(IoSnafu {})?;
                log::trace!("Received {:?}", &data[..len]);

                if let Some(r) = recv.receive_data(&data[..len]) {
                    return r.context(ProtocolSnafu);
                }
            }
        }
    } // impl Master

    fn check_addr_param(
        addr: impl IntoAddress,
        param: impl IntoParameter,
    ) -> Result<(Address, Parameter), Error> {
        Ok((
            addr.into_address().context(InvalidArgumentSnafu)?,
            param.into_parameter().context(InvalidArgumentSnafu)?,
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
        let (addr, param, val) = addr_param_val(43, 1234, 12345);
        let mut master = Master::new();
        let mut x = master.read_parameter(addr, param);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.get_data(), b"\x0444331234\x05");
        let recv = x.data_sent();
        // println!("{:x}", bcc(b"123412345\x03"));
        assert_eq!(
            recv.receive_data(b"\x02123412345\x03\x36")
                .unwrap()
                .unwrap(),
            val
        );
    }

    #[test]
    fn read_again() {
        let (addr, param, _) = addr_param_val(10, 20, 56);
        let mut idle = Master::new();
        idle.read_again = Some((addr, param));
        let send = idle.read_parameter_again(addr, param.next().unwrap());
        assert_eq!(send.get_data(), [ACK]);
    }
}
