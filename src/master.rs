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
            let (new, old): (i16, i16) = (parameter.into(), old_param.into());
            match new - old {
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

/// Provides the receive_data() method for parsing response
/// data from the slaves.
pub trait Receiver<'a>: Sized + private::CreateReceiver<'a> {
    type Response;

    /// Receive and parse data from the bus. Passing a zero length
    /// slice will result in a TransmissionError response.
    ///
    /// No more data should be read when Some(response) is returned.
    fn receive_data(&mut self, data: &[u8]) -> Option<Self::Response>;
}

#[derive(Debug, PartialEq)]
pub enum WriteResponse {
    WriteOk,
    WriteFailed,
    TransmissionError,
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
    type Response = WriteResponse;

    fn receive_data(&mut self, data: &[u8]) -> Option<Self::Response> {
        use ResponseToken::*;

        if data.is_empty() {
            return Some(WriteResponse::TransmissionError);
        }
        self.buffer.write(data);

        match parse_write_reponse(self.buffer.as_str_slice()) {
            NeedData => None,
            WriteOk => Some(WriteResponse::WriteOk),
            WriteFailed | InvalidParameter => Some(WriteResponse::WriteFailed),
            _ => Some(WriteResponse::TransmissionError),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadResponse {
    InvalidParameter,
    Ok(Value),
    TransmissionError,
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
    type Response = ReadResponse;

    fn receive_data(&mut self, data: &[u8]) -> Option<Self::Response> {
        use ResponseToken::*;

        if data.is_empty() {
            return Some(ReadResponse::TransmissionError);
        }

        self.buffer.write(data);

        match parse_read_response(self.buffer.as_str_slice()) {
            NeedData => None,
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
                Some(ReadResponse::Ok(value))
            }
            InvalidParameter => Some(ReadResponse::InvalidParameter),
            _ => Some(ReadResponse::TransmissionError),
        }
    }
}

pub mod io {
    use snafu::{Backtrace, ResultExt, Snafu};

    use crate::master::{ReadResponse, Receiver, SendData, WriteResponse};
    use crate::types::{self, IntoAddress, IntoParameter, Value};

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
            let data_out = self.proto.write_parameter(address, parameter, value);
            let receiver = self.stream.send_data(data_out)?;
            match self.stream.receive_data(receiver) {
                WriteResponse::WriteOk => Ok(()),
                WriteResponse::WriteFailed => WriteNAK {}.fail(),
                WriteResponse::TransmissionError => BusDataError {}.fail(),
            }
        }

        pub fn read_parameter(
            &mut self,
            address: impl IntoAddress,
            parameter: impl IntoParameter,
        ) -> Result<Value, Error> {
            let address = address.into_address()?;
            let parameter = parameter.into_parameter()?;
            let send = self.proto.read_parameter(address, parameter);
            let receiver = self.stream.send_data(send)?;
            match self.stream.receive_data(receiver) {
                ReadResponse::Ok(value) => Ok(value),
                ReadResponse::InvalidParameter => InvalidParameter {}.fail(),
                ReadResponse::TransmissionError => BusDataError {}.fail(),
            }
        }
    } // impl Master

    trait MasterTRX: std::io::Read + std::io::Write {
        fn receive_data<'a, T: Receiver<'a>>(&mut self, receiver: T) -> T::Response;
        fn send_data<'a, T: Receiver<'a>>(&mut self, sender: SendData<'a, T>) -> Result<T, Error>;
    }

    impl<T> MasterTRX for T
    where
        T: std::io::Read + std::io::Write,
    {
        fn receive_data<'a, R: Receiver<'a>>(
            &mut self,
            mut receiver: R,
        ) -> <R as Receiver<'a>>::Response {
            let mut data = [0];
            loop {
                let len = match self.read(&mut data) {
                    Ok(len) => len,
                    _ => 0,
                };
                // A zero length slice will cause receive_data() to return TransmissionError
                if let Some(resp) = receiver.receive_data(&data[..len]) {
                    return resp;
                }
            }
        }

        fn send_data<'a, R: Receiver<'a>>(&mut self, sender: SendData<'a, R>) -> Result<R, Error> {
            match self.write_all(sender.as_slice()).and_then(|_| self.flush()) {
                Ok(_) => Ok(sender.data_sent()),
                Err(err) => {
                    sender.send_failed();
                    Err(err)
                }
            }
            .context(IOError {})
        }
    }
}

/// Tests for the base sans-IO master implementation
#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryInto;

    #[test]
    fn write_parameter() -> Result<(), Error> {
        let mut master = Master::new();
        let x = master.write_parameter(43.try_into()?, 1234.try_into()?, 56);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x044433\x02123400056\x034");
        Ok(())
    }

    #[test]
    fn read_parameter() -> Result<(), Error> {
        let mut master = Master::new();
        let x = master.read_parameter(43.try_into()?, 1234.try_into()?);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x0444331234\x05");
        Ok(())
    }

    #[test]
    fn read_again() -> Result<(), Error> {
        let mut idle = Master::new();
        let addr = Address::new(10)?;
        let param = Parameter::new(20)?;
        idle.set_slave_capabilites(addr, true);
        idle.read_again = Some((addr, param));
        let send = idle.read_parameter(addr, param.checked_add(1)?);
        assert_eq!(send.as_slice(), [ACK.as_byte()]);
        Ok(())
    }
}
