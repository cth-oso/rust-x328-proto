use crate::buffer::Buffer;
use crate::nom_parser::{self, parse_read_response, parse_write_reponse};
use crate::{Address, Parameter, Value, X328Error};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

use crate::slave::bcc;
use ascii::AsciiChar::{ENQ, EOT, ETX, SOX};

type StateT = Box<MasterState>;

pub struct MasterState {
    last_address: Option<Address>,
    read_in_progress: Option<Parameter>,
    slaves: [SlaveState; 100],
}

impl Debug for MasterState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MasterState {{ last_address: {:?}, slaves: [..]}}",
            self.last_address
        )
    }
}

#[derive(Copy, Clone)]
pub struct SlaveState {
    can_read_again: bool,
    can_write_again: bool,
}

#[derive(Debug)]
pub struct Master {
    state: StateT,
}

impl Master {
    pub fn new() -> Master {
        Master {
            state: Box::new(MasterState {
                last_address: None,
                read_in_progress: None,
                slaves: [SlaveState {
                    can_read_again: false,
                    can_write_again: false,
                }; 100],
            }),
        }
    }

    pub fn write_parameter(
        self,
        address: Address,
        parameter: Parameter,
        value: Value,
    ) -> SendData<ReceiveWriteResponse> {
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
        data.push(EOT.as_byte());
        data.extend_from_slice(&address.to_bytes());
        data.extend_from_slice(&parameter.to_bytes());
        data.push(ENQ.as_byte());
        self.state.read_in_progress = Some(parameter);
        SendData::new(self.state, data)
    }
}

impl Default for Master {
    fn default() -> Self {
        Master::new()
    }
}

impl From<StateT> for Master {
    fn from(state: StateT) -> Self {
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
        self.state.last_address = None;
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
        use nom_parser::ResponseToken::*;

        if data.is_empty() {
            return ReceiverResult::Done(self.state.into(), WriteResponse::TransmissionError);
        }

        self.buffer.write(data);
        let (consumed, token) = { parse_write_reponse(self.buffer.as_str_slice()) };
        self.buffer.consume(consumed);
        match token {
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
        use nom_parser::ResponseToken::*;
        use ReceiverResult::Done;

        if data.is_empty() {
            return ReceiverResult::Done(self.state.into(), ReadResponse::TransmissionError);
        }

        self.buffer.write(data);

        let (consumed, token) = { parse_read_response(self.buffer.as_str_slice()) };
        self.buffer.consume(consumed);
        match token {
            NeedData => ReceiverResult::NeedData(self),
            ReadOK { parameter, value }
                if (parameter
                    == self
                        .state
                        .read_in_progress
                        .expect("read_in_progress is None while running read query!")) =>
            {
                Done(self.state.into(), ReadResponse::Ok(value))
            }
            InvalidParameter => Done(self.state.into(), ReadResponse::InvalidParameter),
            _ => Done(self.state.into(), ReadResponse::TransmissionError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slave::tests::{SerialIOPlane, SerialInterface};
    use crate::X328Error::IOError;
    use ascii::AsciiChar::{ACK, NAK};
    use std::collections::HashMap;

    #[derive(Debug)]
    pub struct StreamMaster<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        idle_state: Option<Master>,
        stream: IO,
    }

    impl<IO> StreamMaster<IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        pub fn new(io: IO) -> StreamMaster<IO> {
            StreamMaster {
                idle_state: Master::new().into(),
                stream: io,
            }
        }
        // Sends a write command to the slave. May use the shorter "write again" command form
        pub fn write_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
            value: Value,
        ) -> Result<WriteResponse, X328Error> {
            let idle_state = self.take_idle(); // self.idle_state must be Some at start of call
            let data_out = idle_state.write_parameter(address, parameter, value);
            let receiver = self.send_data(data_out)?;
            Ok(self.receive_data(receiver))
        }

        pub fn read_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
        ) -> Result<ReadResponse, X328Error> {
            let idle = self.take_idle();
            let send = idle.read_parameter(address, parameter);
            let receiver = self.send_data(send)?;
            Ok(self.receive_data(receiver))
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

        fn take_idle(&mut self) -> Master {
            self.idle_state.take().unwrap()
        }
        fn get_idle(&self) -> &Master {
            self.idle_state.as_ref().unwrap()
        }
    }

    #[test]
    fn master_main_loop() {
        let data_in = [SOX.as_byte(), ACK.as_byte()];
        let serial_sim = SerialInterface::new(&data_in);
        let mut serial = SerialIOPlane::new(&serial_sim);
        // let mut registers: HashMap<Parameter, Value> = HashMap::new();

        let mut master = StreamMaster::new(&mut serial);
        let addr10 = Address::new_unchecked(10);
        let param20 = Parameter::new_unchecked(20);
        let x = master.write_parameter(addr10, param20, 3);
        assert_eq!(x.unwrap(), WriteResponse::TransmissionError);
        serial_sim.borrow_mut().trigger_write_error();
        let x = master.write_parameter(addr10, param20, 3);
        assert_eq!(x.unwrap_err(), IOError);
        let x = master.write_parameter(addr10, param20, 3);
        println!("Write success: {:?}", x);
        assert_eq!(x.unwrap(), WriteResponse::WriteOk);
    }
}
