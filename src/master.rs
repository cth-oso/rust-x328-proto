//! The bus controller half of the X3.28 protocol
use arrayvec::ArrayVec;

use std::fmt::{Debug, Formatter};

use crate::ascii::*;
use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::master::{parse_read_response, parse_write_response, ResponseToken};
use crate::types::{Address, Parameter, Value};
use std::marker::PhantomData;

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
        Master::new()
    }
}

impl Master {
    pub fn new() -> Master {
        Master {
            read_again: None,
            nodes: [NodeState {
                can_read_again: false,
            }; 100],
        }
    }

    /// Initiate a write command to a node.
    ///
    /// The returned struct holds the data that needs to be transmitted
    /// on the bus.
    pub fn write_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> SendData<ReceiveWriteResponse, WriteResult> {
        self.read_again = None;
        let mut data = SendDataStore::new();
        data.push(EOT);
        data.try_extend_from_slice(&address.to_bytes()).unwrap();
        data.push(STX);
        data.try_extend_from_slice(&parameter.to_bytes()).unwrap();
        data.try_extend_from_slice(&value.to_bytes()).unwrap();
        data.push(ETX);
        data.push(bcc(&data[6..]));
        SendData::new(data, ReceiveWriteResponse::new(self))
    }

    /// Initiate a read command to a node.
    ///
    /// The returned [SendData] struct holds the data that needs to be transmitted
    /// on the bus.
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
    /// Consumes the self.read_again value
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

    pub fn set_node_capabilities(&mut self, address: Address, can_read_again: bool) {
        self.nodes[address.as_usize()] = NodeState { can_read_again };
    }

    fn get_node_capabilities(&self, address: Address) -> NodeState {
        self.nodes[address.as_usize()]
    }
}

type SendDataStore = ArrayVec<u8, 20>;

/// [SendData] holds data that should be transmitted to the nodes.
///
/// Call [data_sent()](Self::data_sent()) after the data has been
/// successfully transmitted in order to transition to the "response
/// receive" state. If data transmission fails this struct should be
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

/// Return value from Receiver::receive_data()
/// Indicates if enough data has been received or if more data is needed.
/// R is the receiver (Self), T is Self::Response
pub enum ReceiveDataResult<R, T> {
    Done(T),
    NeedData(R),
}

/// Provides the receive_data() method for parsing response
/// data from the nodes.
pub trait Receiver<Response>: Sized + private::Receiver {
    /// Receive and parse data from the bus.
    ///
    /// Note that the method consumes self, so it must be reclaimed
    /// from the return value.
    fn receive_data(self, data: &[u8]) -> ReceiveDataResult<Self, Response>;
}

#[derive(Debug, PartialEq)]
pub enum WriteResult {
    WriteOk,
    WriteFailed,
    ProtocolError,
}

/// Call [receive_data()](Receiver::receive_data()) to process the
/// received response data from the node.
#[derive(Debug)]
pub struct ReceiveWriteResponse<'a> {
    master: &'a mut Master,
    buffer: Buffer,
}

impl<'a> ReceiveWriteResponse<'a> {
    fn new(master: &'a mut Master) -> Self {
        ReceiveWriteResponse {
            master,
            buffer: Buffer::new(),
        }
    }
}

impl private::Receiver for ReceiveWriteResponse<'_> {}

impl Receiver<WriteResult> for ReceiveWriteResponse<'_> {
    fn receive_data(mut self, data: &[u8]) -> ReceiveDataResult<Self, WriteResult> {
        use ResponseToken::*;
        self.buffer.write(data);

        ReceiveDataResult::Done(match parse_write_response(self.buffer.as_ref()) {
            WriteOk => WriteResult::WriteOk,
            WriteFailed | InvalidParameter => WriteResult::WriteFailed,
            _ => WriteResult::ProtocolError,
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadResult {
    InvalidParameter,
    Ok(Value),
    ProtocolError,
}

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
    fn receive_data(mut self, data: &[u8]) -> ReceiveDataResult<Self, ReadResult> {
        use ResponseToken::*;
        self.buffer.write(data);

        ReceiveDataResult::Done(match parse_read_response(self.buffer.as_ref()) {
            NeedData => return ReceiveDataResult::NeedData(self),
            ReadOK { parameter, value } if (parameter == self.expected_param) => {
                self.master.read_again = Some((self.address, parameter));
                ReadResult::Ok(value)
            }
            InvalidParameter => ReadResult::InvalidParameter,
            _ => ReadResult::ProtocolError,
        })
    }
}

pub mod io {
    use snafu::{Backtrace, OptionExt, ResultExt, Snafu};

    use crate::master::{ReadResult, ReceiveDataResult, Receiver, SendData, WriteResult};
    use crate::types::{IntoAddress, IntoParameter, IntoValue, Value};
    use crate::{Address, Parameter};
    use std::io::{Read, Write};

    #[derive(Debug, Snafu)]
    pub enum Error {
        #[snafu(display("Invalid argument: {} out of range", arg))]
        InvalidArgument { arg: &'static str },
        #[snafu(display("X3.28 invalid parameter"))]
        InvalidParameter { backtrace: Backtrace },
        #[snafu(display("X3.28 write received NAK response"))]
        WriteFailed { backtrace: Backtrace },
        #[snafu(display("X3.28 error: bad transmission."))]
        BusDataError { backtrace: Backtrace },
        #[snafu(display("X3.28 IO error: {}", source))]
        IoError {
            source: std::io::Error,
            backtrace: Backtrace,
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
                    ReceiveDataResult::Done(response) => return Ok(response),
                    ReceiveDataResult::NeedData(reader) => self = reader,
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
        pub fn new(io: IO) -> Master<IO> {
            Master {
                proto: super::Master::new(),
                stream: io,
            }
        }

        pub fn set_can_read_again(&mut self, address: impl IntoAddress, value: bool) {
            self.proto
                .set_node_capabilities(address.into_address().unwrap(), value);
        }

        /// Sends a write command to the node. May use the shorter "write again" command form
        pub fn write_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
            value: impl IntoValue,
        ) -> Result<(), Error> {
            let (address, parameter) = check_addr_param(address, parameter)?;
            let value = value
                .into_value()
                .context(InvalidArgument { arg: "value" })?;
            let response = self
                .proto
                .write_parameter(address, parameter, value)
                .write_to(&mut self.stream)?
                .receive_from(&mut self.stream)?;
            match response {
                WriteResult::WriteOk => Ok(()),
                WriteResult::WriteFailed => WriteFailed {}.fail(),
                WriteResult::ProtocolError => BusDataError {}.fail(),
            }
        }

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
            match response {
                ReadResult::Ok(value) => Ok(value),
                ReadResult::InvalidParameter => InvalidParameter {}.fail(),
                ReadResult::ProtocolError => BusDataError {}.fail(),
            }
        }
    } // impl Master

    fn check_addr_param(
        addr: impl IntoAddress,
        param: impl IntoParameter,
    ) -> Result<(Address, Parameter), Error> {
        Ok((
            addr.into_address()
                .ok()
                .context(InvalidArgument { arg: "address" })?,
            param
                .into_parameter()
                .ok()
                .context(InvalidArgument { arg: "parameter" })?,
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
        idle.set_node_capabilities(addr, true);
        idle.read_again = Some((addr, param));
        let send = idle.read_parameter(addr, param.checked_add(1).unwrap());
        assert_eq!(send.get_data(), [ACK]);
    }
}
