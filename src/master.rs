use crate::buffer::Buffer;
use crate::nom_parser::{self, parse_reponse};
use crate::{Address, Parameter, Value, X328Error};
use std::marker::PhantomData;

type StateT = Box<MasterState>;

pub struct MasterState {
    last_address: Option<Address>,
    slaves: [SlaveState; 100],
}

#[derive(Copy, Clone)]
pub struct SlaveState {
    can_read_again: bool,
    can_write_again: bool,
}

pub struct Master {
    state: StateT,
}

impl Master {
    pub fn new() -> Master {
        Master {
            state: Box::new(MasterState {
                last_address: None,
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
        unimplemented!();
        //SendData { data: vec![0] }
    }

    pub fn read_parameter(
        self,
        address: Address,
        parameter: Parameter,
    ) -> SendData<ReceiveReadResponse> {
        unimplemented!()
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

pub struct SendData<R: Receiver<R>> {
    state: StateT,
    data: Vec<u8>,
    receiver: PhantomData<R>,
}

impl<R: Receiver<R>> SendData<R> {
    pub fn as_slice(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn data_sent(self) -> R {
        R::new(self.state)
    }
}

pub enum ReceiverResult<R, T> {
    NeedData(R),
    Done(Master, T),
}

pub trait Receiver<R: Receiver<R>> {
    type Response;
    fn new(state: StateT) -> Self;
    fn receive_data(self, data: &[u8]) -> ReceiverResult<R, Self::Response>;
}

pub enum WriteResponse {
    ACK,
    NAK,
    TransmissionError,
}

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
        self.buffer.write(data);
        use nom_parser::ResponseToken::*;
        let (consumed, token) = { parse_reponse(self.buffer.as_str_slice()) };
        self.buffer.consume(consumed);
        match token {
            NeedData => ReceiverResult::NeedData(self),
            WriteOk => ReceiverResult::Done(self.state.into(), WriteResponse::ACK),
            WriteFailed | InvalidParameter => {
                ReceiverResult::Done(self.state.into(), WriteResponse::NAK)
            }
            _ => ReceiverResult::Done(self.state.into(), WriteResponse::TransmissionError),
        }
    }
}

pub enum ReadResponse {
    InvalidParameter,
    Ok(Value),
    TransmissionError,
}

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
        self.buffer.write(data);
        use nom_parser::ResponseToken::*;
        let (consumed, token) = { parse_reponse(self.buffer.as_str_slice()) };
        self.buffer.consume(consumed);
        match token {
            NeedData => ReceiverResult::NeedData(self),
            ReadOK { parameter, value } => {
                ReceiverResult::Done(self.state.into(), ReadResponse::Ok(value))
            }
            ReadFailed => unimplemented!(),
            InvalidParameter => unimplemented!(),
            _ => ReceiverResult::Done(self.state.into(), ReadResponse::TransmissionError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slave::tests::SerialInterface;
    use std::collections::HashMap;

    struct StreamMaster<'a, IO>
// where IO: std::io::Read + std::io::Write
    {
        idle_state: Option<Master>,
        stream: &'a mut IO,
    }

    impl<IO> StreamMaster<'_, IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        // Sends a write command to the slave. May use the shorter "write again" command form
        pub fn write_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
            value: Value,
        ) -> Result<WriteResponse, X328Error> {
            let idle_state = self.take_idle(); // self.idle_state must be Some at start of call
            let data_out = idle_state.write_parameter(address, parameter, value);

            self.send_data(data_out.as_slice())?; // FIXME: handle error state

            Ok(self.receive_data(data_out.data_sent()))
        }

        pub fn read_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
        ) -> Result<ReadResponse, X328Error> {
            let idle = self.take_idle();
            let send = idle.read_parameter(address, parameter);
            self.send_data(send.as_slice())?;
            Ok(self.receive_data(send.data_sent()))
        }

        fn send_data(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.stream.write_all(data)
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
        let data_in = b"asd";
        let mut serial = SerialInterface::new(data_in);
        let mut registers: HashMap<Parameter, Value> = HashMap::new();

        let mut master_proto = Master::new();
    }
}
