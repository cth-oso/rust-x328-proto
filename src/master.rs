use crate::buffer::Buffer;
use crate::nom_parser::master::{parse_read_response, parse_write_reponse, ResponseToken};
use crate::{Address, Parameter, Value};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

use crate::slave::bcc;
use ascii::AsciiChar::{BackSpace, ACK, ENQ, EOT, ETX, NAK, SOX};

type StateT = Box<MasterState>;

pub struct MasterState {
    read_again: Option<(Address, Parameter)>,
    read_in_progress: Option<(Address, Parameter)>,
    slaves: [SlaveState; 100],
}

impl Debug for MasterState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MasterState {{ read_again: {:?}, slaves: [..]}}",
            self.read_again
        )
    }
}

#[derive(Copy, Clone)]
pub struct SlaveState {
    can_read_again: bool,
}

#[derive(Debug)]
pub struct Master {
    state: StateT,
}

impl Master {
    pub fn new() -> Master {
        Master {
            state: Box::new(MasterState {
                read_again: None,
                read_in_progress: None,
                slaves: [SlaveState {
                    can_read_again: false,
                }; 100],
            }),
        }
    }

    pub fn write_parameter(
        mut self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> SendData<ReceiveWriteResponse> {
        self.state.read_again = None;
        let mut data = Vec::with_capacity(20);
        data.push(EOT.as_byte());
        data.extend_from_slice(&address.to_bytes());
        data.push(SOX.as_byte());
        data.extend_from_slice(&parameter.to_bytes());
        data.extend_from_slice(format!("{:05}", value).as_bytes());
        data.push(ETX.as_byte());
        data.push(bcc(&data[6..]));
        SendData::new(self.state, data)
    }

    pub fn read_parameter(
        mut self,
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
        self.state.read_in_progress = Some((address, parameter));
        SendData::new(self.state, data)
    }

    fn read_again(&mut self, address: Address, parameter: Parameter) -> Option<u8> {
        let (old_addr, old_param) = self.state.read_again.take()?;
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
        self.state.slaves[address.as_usize()] = SlaveState { can_read_again };
    }

    fn get_slave_capabilites(&self, address: Address) -> SlaveState {
        self.state.slaves[address.as_usize()]
    }
}

impl Default for Master {
    fn default() -> Self {
        Master::new()
    }
}

impl From<StateT> for Master {
    fn from(mut state: StateT) -> Self {
        state.read_in_progress = None;
        Master { state }
    }
}

#[derive(Debug)]
pub struct SendData<R: Receiver<R>> {
    state: StateT,
    data: Vec<u8>,
    receiver: PhantomData<R>,
}

impl<R: Receiver<R>> SendData<R> {
    fn new(state: StateT, data: Vec<u8>) -> Self {
        SendData {
            state,
            data,
            receiver: PhantomData,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn data_sent(self) -> R {
        R::new(self.state)
    }

    pub fn send_failed(mut self) -> Master {
        self.state.read_again = None;
        self.state.read_in_progress = None;
        Master::from(self.state)
    }
}

#[derive(Debug)]
pub enum ReceiverResult<R, T> {
    NeedData(R),
    Done(Master, T),
}

pub trait Receiver<R: Receiver<R>> {
    type Response;
    fn new(state: StateT) -> Self;
    fn receive_data(self, data: &[u8]) -> ReceiverResult<R, Self::Response>;
}

#[derive(Debug, PartialEq)]
pub enum WriteResponse {
    WriteOk,
    WriteFailed,
    TransmissionError,
}

#[derive(Debug)]
pub struct ReceiveWriteResponse {
    state: StateT,
    buffer: Buffer,
}

impl Receiver<ReceiveWriteResponse> for ReceiveWriteResponse {
    type Response = WriteResponse;
    fn new(state: StateT) -> ReceiveWriteResponse {
        ReceiveWriteResponse {
            state,
            buffer: Buffer::new(),
        }
    }

    fn receive_data(mut self, data: &[u8]) -> ReceiverResult<ReceiveWriteResponse, Self::Response> {
        use ResponseToken::*;

        if data.is_empty() {
            return ReceiverResult::Done(self.state.into(), WriteResponse::TransmissionError);
        }
        self.buffer.write(data);

        match parse_write_reponse(self.buffer.as_str_slice()) {
            NeedData => ReceiverResult::NeedData(self),
            WriteOk => ReceiverResult::Done(self.state.into(), WriteResponse::WriteOk),
            WriteFailed | InvalidParameter => {
                ReceiverResult::Done(self.state.into(), WriteResponse::WriteFailed)
            }
            _ => ReceiverResult::Done(self.state.into(), WriteResponse::TransmissionError),
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
pub struct ReceiveReadResponse {
    state: StateT,
    buffer: Buffer,
}

impl Receiver<ReceiveReadResponse> for ReceiveReadResponse {
    type Response = ReadResponse;
    fn new(state: StateT) -> ReceiveReadResponse {
        ReceiveReadResponse {
            state,
            buffer: Buffer::new(),
        }
    }

    fn receive_data(mut self, data: &[u8]) -> ReceiverResult<ReceiveReadResponse, Self::Response> {
        use ReceiverResult::Done;
        use ResponseToken::*;

        if data.is_empty() {
            return ReceiverResult::Done(self.state.into(), ReadResponse::TransmissionError);
        }

        self.buffer.write(data);

        match parse_read_response(self.buffer.as_str_slice()) {
            NeedData => ReceiverResult::NeedData(self),
            ReadOK { parameter, value }
                if (parameter
                    == self
                        .state
                        .read_in_progress
                        .expect("read_in_progress is None while running read query!")
                        .1) =>
            {
                self.state.read_again = self.state.read_in_progress;
                debug_assert!(self.state.read_again.is_some());
                Done(self.state.into(), ReadResponse::Ok(value))
            }
            InvalidParameter => Done(self.state.into(), ReadResponse::InvalidParameter),
            _ => Done(self.state.into(), ReadResponse::TransmissionError),
        }
    }
}

pub mod io {
    use crate::master::{ReadResponse, Receiver, ReceiverResult, SendData, WriteResponse};
    use crate::{Address, Parameter, Value, X328Error};

    #[derive(Debug)]
    pub struct Master<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        idle_state: Option<super::Master>,
        stream: IO,
    }

    impl<IO> Master<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        pub fn new(io: IO) -> Master<IO> {
            Master {
                idle_state: super::Master::new().into(),
                stream: io,
            }
        }

        pub fn set_can_read_again(&mut self, address: Address, value: bool) {
            self.idle_state
                .as_mut()
                .unwrap()
                .set_slave_capabilites(address, value);
        }

        // Sends a write command to the slave. May use the shorter "write again" command form
        pub fn write_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
            value: Value,
        ) -> Result<(), X328Error> {
            let idle_state = self.take_idle(); // self.idle_state must be Some at start of call
            let data_out = idle_state.write_parameter(address, parameter, value);
            let receiver = self.send_data(data_out)?;
            match self.receive_data(receiver) {
                WriteResponse::WriteOk => Ok(()),
                WriteResponse::WriteFailed => Err(X328Error::WriteNAK),
                WriteResponse::TransmissionError => Err(X328Error::IOError),
            }
        }

        pub fn read_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
        ) -> Result<Value, X328Error> {
            let idle = self.take_idle();
            let send = idle.read_parameter(address, parameter);
            let receiver = self.send_data(send)?;
            match self.receive_data(receiver) {
                ReadResponse::Ok(value) => Ok(value),
                ReadResponse::InvalidParameter => Err(X328Error::InvalidParameter),
                ReadResponse::TransmissionError => Err(X328Error::IOError),
            }
        }

        fn send_data<T: Receiver<T>>(&mut self, sender: SendData<T>) -> Result<T, X328Error> {
            if let Err(_err) = self.stream.write_all(sender.as_slice()) {
                self.idle_state = Some(sender.send_failed());
                Err(X328Error::IOError)
            } else {
                Ok(sender.data_sent())
            }
        }

        fn receive_data<T: Receiver<T>>(&mut self, mut receiver: T) -> T::Response {
            let mut data = [0];
            loop {
                match if let Ok(len) = self.stream.read(&mut data) {
                    receiver.receive_data(&data[..len])
                } else {
                    receiver.receive_data(&[] as &[u8])
                } {
                    ReceiverResult::NeedData(new_receiver) => receiver = new_receiver,
                    ReceiverResult::Done(idle, resp) => {
                        self.idle_state = Some(idle);
                        return resp;
                    }
                };
            }
        }

        fn take_idle(&mut self) -> super::Master {
            self.idle_state.take().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::X328Error;
    use std::convert::TryInto;

    #[test]
    fn write_parameter() -> Result<(), X328Error> {
        let x = Master::new().write_parameter(43.try_into()?, 1234.try_into()?, 56);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x044433\x02123400056\x034");
        Ok(())
    }

    #[test]
    fn read_parameter() -> Result<(), X328Error> {
        let x = Master::new().read_parameter(43.try_into()?, 1234.try_into()?);
        // println!("{}", String::from_utf8(x.as_slice().to_vec()).unwrap());
        assert_eq!(x.as_slice(), b"\x0444331234\x05");
        Ok(())
    }

    #[test]
    fn read_again() -> Result<(), X328Error> {
        let mut idle = Master::new();
        let addr = Address::new(10)?;
        let param = Parameter::new(20)?;
        idle.set_slave_capabilites(addr, true);
        idle.state.read_again = Some((addr, param));
        let send = idle.read_parameter(addr, param.checked_add(1)?);
        assert_eq!(send.as_slice(), [ACK.as_byte()]);
        Ok(())
    }
}
