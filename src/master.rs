use ascii::AsciiChar::{BackSpace, ACK, ENQ, EOT, ETX, NAK, SOX};
use snafu::Snafu;

use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

use crate::bcc;
use crate::buffer::Buffer;
use crate::nom_parser::master::{parse_read_response, parse_write_reponse, ResponseToken};
use crate::types::{self, Address, Parameter, Value};

#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    #[snafu(display("Invalid argument {}", source), context(false))]
    InvalidArgument { source: types::Error },
}

#[derive(Copy, Clone)]
struct SlaveState {
    can_read_again: bool,
}

pub struct Master {
    read_again: Option<(Address, Parameter)>,
    read_in_progress: Option<(Address, Parameter)>,
    slaves: [SlaveState; 100],
}

impl Debug for Master {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Master {{ read_again: {:?}, read_in_progress: {:?}, slaves: [..]}}",
            self.read_again, self.read_in_progress
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
            read_in_progress: None,
            slaves: [SlaveState {
                can_read_again: false,
            }; 100],
        }
    }

    pub fn write_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> SendData<ReceiveWriteResponse<'_>> {
        self.read_again = None;
        let mut data = Vec::with_capacity(20);
        data.push(EOT.as_byte());
        data.extend_from_slice(&address.to_bytes());
        data.push(SOX.as_byte());
        data.extend_from_slice(&parameter.to_bytes());
        data.extend_from_slice(format!("{:05}", value).as_bytes());
        data.push(ETX.as_byte());
        data.push(bcc(&data[6..]));
        SendData::new(self, data)
    }

    pub fn read_parameter(
        &mut self,
        address: Address,
        parameter: Parameter,
    ) -> SendData<ReceiveReadResponse> {
        let mut data = Vec::with_capacity(10);
        if let Some(again) = self.read_again(address, parameter) {
            data.push(again);
        } else {
            data.push(EOT.as_byte());
            data.extend_from_slice(&address.to_bytes());
            data.extend_from_slice(&parameter.to_bytes());
            data.push(ENQ.as_byte());
        }
        self.read_in_progress = Some((address, parameter));
        SendData::new(self, data)
    }

    fn read_again(&mut self, address: Address, parameter: Parameter) -> Option<u8> {
        let (old_addr, old_param) = self.read_again.take()?;
        if old_addr == address && self.get_slave_capabilites(address).can_read_again {
            match *parameter - *old_param {
                0 => Some(NAK.as_byte()),
                1 => Some(ACK.as_byte()),
                -1 => Some(BackSpace.as_byte()),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn set_slave_capabilites(&mut self, address: Address, can_read_again: bool) {
        self.slaves[address.as_usize()] = SlaveState { can_read_again };
    }

    fn get_slave_capabilites(&self, address: Address) -> SlaveState {
        self.slaves[address.as_usize()]
    }
}

#[derive(Debug)]
pub struct SendData<'a, R: Receiver<'a>> {
    master: &'a mut Master,
    data: Vec<u8>,
    receiver: PhantomData<R>,
}

impl<'a, R: Receiver<'a>> SendData<'a, R> {
    fn new(master: &'a mut Master, data: Vec<u8>) -> Self {
        SendData {
            master,
            data,
            receiver: PhantomData,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn data_sent(self) -> R {
        R::new(self.master)
    }

    pub fn send_failed(mut self) {
        self.master.read_again = None;
        self.master.read_in_progress = None;
    }
}

mod private {
    use super::Master;
    pub trait CreateReceiver<'a> {
        fn new(master: &'a mut Master) -> Self;
    }
}

/// Return value from Receiver::recieve_data()
/// Indicates if enough data has been received or if more data is needed.
/// R is the receiver (Self), T is Self::Response
pub enum ReceiveDataResult<R, T> {
    Done(T),
    NeedData(R),
}

/// Provides the receive_data() method for parsing response
/// data from the slaves.
pub trait Receiver<'a>: Sized + private::CreateReceiver<'a> {
    type Response;

    /// Receive and parse data from the bus. Passing a zero length
    /// slice will result in a TransmissionError response.
    ///
    /// No more data should be read when Some(response) is returned.
    fn receive_data(self, data: &[u8]) -> ReceiveDataResult<Self, Self::Response>;
}

#[derive(Debug, PartialEq)]
pub enum WriteResult {
    WriteOk,
    WriteFailed,
    ProtocolError,
}

#[derive(Debug)]
pub struct ReceiveWriteResponse<'a> {
    master: &'a Master,
    buffer: Buffer,
}

impl<'a> private::CreateReceiver<'a> for ReceiveWriteResponse<'a> {
    fn new(master: &'a mut Master) -> ReceiveWriteResponse<'a> {
        ReceiveWriteResponse {
            master,
            buffer: Buffer::new(),
        }
    }
}

impl<'b> Receiver<'b> for ReceiveWriteResponse<'b> {
    type Response = WriteResult;

    fn receive_data(mut self, data: &[u8]) -> ReceiveDataResult<Self, Self::Response> {
        use ResponseToken::*;

        if data.is_empty() {
            return ReceiveDataResult::Done(WriteResult::ProtocolError);
        }
        self.buffer.write(data);

        ReceiveDataResult::Done(match parse_write_reponse(self.buffer.as_str_slice()) {
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
}

impl<'a> private::CreateReceiver<'a> for ReceiveReadResponse<'a> {
    fn new(master: &'a mut Master) -> ReceiveReadResponse<'a> {
        ReceiveReadResponse {
            master,
            buffer: Buffer::new(),
        }
    }
}

impl<'a> Receiver<'a> for ReceiveReadResponse<'a> {
    type Response = ReadResult;

    fn receive_data(mut self, data: &[u8]) -> ReceiveDataResult<Self, Self::Response> {
        use ResponseToken::*;

        if data.is_empty() {
            return ReceiveDataResult::Done(ReadResult::ProtocolError);
        }

        self.buffer.write(data);

        ReceiveDataResult::Done(match parse_read_response(self.buffer.as_str_slice()) {
            NeedData => return ReceiveDataResult::NeedData(self),
            ReadOK { parameter, value }
                if (parameter
                    == self
                        .master
                        .read_in_progress
                        .expect("read_in_progress is None while running read query!")
                        .1) =>
            {
                self.master.read_again = self.master.read_in_progress;
                debug_assert!(self.master.read_again.is_some());
                ReadResult::Ok(value)
            }
            InvalidParameter => ReadResult::InvalidParameter,
            _ => ReadResult::ProtocolError,
        })
    }
}

pub mod io {
    use snafu::{Backtrace, ResultExt, Snafu};

    use crate::master::{ReadResult, ReceiveDataResult, Receiver, SendData, WriteResult};
    use crate::types::{self, IntoAddress, IntoParameter, Value};
    use std::io::{Read, Write};

    #[derive(Debug, Snafu)]
    pub enum Error {
        #[snafu(display("Invalid argument given: {}", source), context(false))]
        InvalidArgument { source: types::Error },
        #[snafu(display("X3.28 invalid parameter"))]
        InvalidParameter { backtrace: Backtrace },
        #[snafu(display("X3.28 write received NAK response"))]
        WriteNAK { backtrace: Backtrace },
        #[snafu(display("X3.28 error: bad transmission."))]
        BusDataError { backtrace: Backtrace },
        #[snafu(display("X3.28 IO error: {}", source))]
        IOError {
            source: std::io::Error,
            backtrace: Backtrace,
        },
    }

    trait ReceiveFrom<'a>: Receiver<'a> {
        fn receive_from(self, reader: &mut impl Read) -> Self::Response;
    }

    impl<'a, R: Receiver<'a>> ReceiveFrom<'a> for R {
        fn receive_from(mut self, reader: &mut impl Read) -> Self::Response {
            let mut data = [0];
            loop {
                let len = reader.read(&mut data).unwrap_or(0);
                // A zero length slice will cause receive_data() to return TransmissionError
                match self.receive_data(&data[..len]) {
                    ReceiveDataResult::Done(response) => return response,
                    ReceiveDataResult::NeedData(reader) => self = reader,
                }
            }
        }
    }

    trait WriteData<R> {
        fn write_to(self, writer: &mut impl std::io::Write) -> Result<R, Error>;
    }

    impl<'a, R> WriteData<R> for SendData<'a, R>
    where
        R: Receiver<'a>,
    {
        fn write_to(self, writer: &mut impl Write) -> Result<R, Error> {
            match writer
                .write_all(self.as_slice())
                .and_then(|_| writer.flush())
            {
                Ok(_) => Ok(self.data_sent()),
                Err(err) => {
                    self.send_failed();
                    Err(err)
                }
            }
            .context(IOError {})
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
                .set_slave_capabilites(address.into_address().unwrap(), value);
        }

        // Sends a write command to the slave. May use the shorter "write again" command form
        pub fn write_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
            value: Value,
        ) -> Result<(), Error> {
            let address = address.into_address()?;
            let parameter = parameter.into_parameter()?;
            let response = self
                .proto
                .write_parameter(address, parameter, value)
                .write_to(&mut self.stream)?
                .receive_from(&mut self.stream);
            match response {
                WriteResult::WriteOk => Ok(()),
                WriteResult::WriteFailed => WriteNAK {}.fail(),
                WriteResult::ProtocolError => BusDataError {}.fail(),
            }
        }

        pub fn read_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
        ) -> Result<Value, Error> {
            let address = address.into_address()?;
            let parameter = parameter.into_parameter()?;
            let response = self
                .proto
                .read_parameter(address, parameter)
                .write_to(&mut self.stream)?
                .receive_from(&mut self.stream);
            match response {
                ReadResult::Ok(value) => Ok(value),
                ReadResult::InvalidParameter => InvalidParameter {}.fail(),
                ReadResult::ProtocolError => BusDataError {}.fail(),
            }
        }
    } // impl Master
}

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
    fn write_parameter() -> Result<(), Error> {
        let (addr, param, val) = addr_param_val(43, 1234, 56);
        let mut master = Master::new();
        let x = master.write_parameter(addr, param, val);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x044433\x02123400056\x034");
        Ok(())
    }

    #[test]
    fn read_parameter() -> Result<(), Error> {
        let (addr, param, _) = addr_param_val(43, 1234, 56);
        let mut master = Master::new();
        let x = master.read_parameter(addr, param);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x0444331234\x05");
        Ok(())
    }

    #[test]
    fn read_again() -> Result<(), Error> {
        let (addr, param, _) = addr_param_val(10, 20, 56);
        let mut idle = Master::new();
        idle.set_slave_capabilites(addr, true);
        idle.read_again = Some((addr, param));
        let send = idle.read_parameter(addr, param.checked_add(1)?);
        assert_eq!(send.as_slice(), [ACK.as_byte()]);
        Ok(())
    }
}
