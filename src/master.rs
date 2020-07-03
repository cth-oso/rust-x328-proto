use crate::buffer::Buffer;
use crate::nom_parser::{self, parse_reponse};
use crate::{Address, Parameter, Value, X328Error};
use std::cell::RefCell;
use std::rc::Rc;

pub enum MasterEnum {
    Idle(Idle),
    SendData(SendData),
    ReceiveData(ReceiveData),
    ReadReply(ReadReply),
    WriteReply(WriteReply),
}

struct MasterState {
    last_address: Option<Address>,
    slaves: [SlaveState; 100],
}

#[derive(Copy, Clone)]
struct SlaveState {
    can_read_again: bool,
    can_write_again: bool,
}

pub struct Idle {
    state: Rc<RefCell<MasterState>>,
}

pub struct SendData {
    data: Vec<u8>,
}

pub struct ReceiveData {
    buffer: Buffer,
}

pub struct ReadReply;

pub struct WriteReply {
    success: bool,
}

impl MasterEnum {
    pub fn new() -> MasterEnum {
        MasterEnum::Idle(Idle {
            state: Rc::new(RefCell::new(MasterState {
                last_address: None,
                slaves: [SlaveState {
                    can_read_again: false,
                    can_write_again: false,
                }; 100],
            })),
        })
    }
}

impl Idle {
    pub fn write_parameter(self, address: Address, parameter: Parameter, value: Value) -> SendData {
        SendData { data: vec![0] }
    }

    pub fn read_parameter(self, address: Address, parameter: Parameter) -> MasterEnum {
        unimplemented!()
    }
}

impl SendData {
    pub fn get_data(&mut self) -> Vec<u8> {
        self.data.split_off(0)
    }
    pub fn data_sent(self) -> ReceiveData {
        ReceiveData {
            buffer: Buffer::new(),
        }
    }
}

impl ReceiveData {
    pub fn receive_data(mut self, data: &[u8]) -> MasterEnum {
        self.buffer.write(data);
        self.parse_buffer()
    }

    pub fn expect_write_response(mut self, data: &[u8]) -> Result<WriteReply, MasterEnum> {
        match self.receive_data(data) {
            MasterEnum::WriteReply(reply) => Ok(reply),
            x => Err(x),
        }
    }

    fn parse_buffer(mut self) -> MasterEnum {
        use nom_parser::ResponseToken::*;
        let (consumed, token) = { parse_reponse(self.buffer.as_str_slice()) };
        self.buffer.consume(consumed);

        match token {
            NeedData => MasterEnum::ReceiveData(self),
            WriteOk => MasterEnum::WriteReply(WriteReply { success: true }),
            WriteFailed => MasterEnum::WriteReply(WriteReply { success: false }),
            ReadOK { parameter, value } => MasterEnum::ReadReply(ReadReply),
            InvalidParameter => MasterEnum::ReadReply(ReadReply),
            ReadFailed => MasterEnum::ReadReply(ReadReply),
        }
    }
}

impl WriteReply {
    pub fn get_result(&self) -> bool {
        self.success
    }
}

/*
impl Into<Idle> for WriteReply {
    fn into(self) -> Idle {
        Idle
    }
}
*/

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::min;
    use std::collections::HashMap;
    use std::io;

    struct SerialInterface {
        rx: Vec<u8>,
        rx_pos: usize,
        tx: Vec<u8>,
    }

    impl SerialInterface {
        fn new(tx: &[u8]) -> SerialInterface {
            SerialInterface {
                tx: tx.to_vec(),
                rx: Vec::new(),
                rx_pos: 0,
            }
        }

        // Will return up to len bytes, until the rx buffer is exhausted
        fn read(&mut self, len: usize) -> Option<&[u8]> {
            let pos = self.rx_pos;
            let new_pos = min(pos + len, self.rx.len());
            if pos == new_pos {
                None
            } else {
                self.rx_pos = new_pos;
                Some(&self.rx[pos..new_pos])
            }
        }
        // Append bytes to the tx buffer
        fn write(&mut self, bytes: &[u8]) {
            self.tx.extend_from_slice(bytes);
        }
    }

    struct StreamMaster<'a, IO>
// where IO: std::io::Read + std::io::Write
    {
        idle_state: Option<Idle>,
        stream: &'a mut IO,
    }

    impl<IO> StreamMaster<'_, IO>
    where
        IO: std::io::Read + std::io::Write,
    {
        pub fn write_parameter(
            &mut self,
            address: Address,
            parameter: Parameter,
            value: Value,
        ) -> Result<bool, X328Error> {
            let mut send = self.take_idle().write_parameter(address, parameter, value);
            self.send_data(send.get_data().as_slice())?; // FIXME: handle error state
            let recv = send.data_sent();
            let response = self.receive_data(recv)?;
            if let MasterEnum::WriteReply(reply) = response {
                Ok(reply.success)
            } else {
                Err(X328Error::OtherError) // unexpected reply
            }
        }

        fn send_data(&mut self, mut data: &[u8]) -> std::io::Result<usize> {
            loop {
                return match self.stream.write(data) {
                    Ok(sent) if sent == data.len() => Ok(data.len()),
                    Ok(sent) if sent > 0 => {
                        data = data[sent..].as_ref();
                        continue;
                    }
                    Ok(_) => Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Zero length write",
                    )),
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(x) => Err(x),
                };
            }
        }

        fn receive_data(&mut self, mut recv: ReceiveData) -> io::Result<MasterEnum> {
            loop {
                let mut data = [0];
                return match self.stream.read(&mut data) {
                    Ok(0) => Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Zero length read",
                    )),
                    Ok(_) => {
                        let master = recv.receive_data(&data);
                        if let MasterEnum::ReceiveData(new_recv) = master {
                            recv = new_recv;
                            continue;
                        }
                        Ok(master)
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => Err(err),
                };
            }
        }

        fn take_idle(&mut self) -> Idle {
            self.idle_state.take().unwrap()
        }
        fn get_idle(&self) -> &Idle {
            self.idle_state.as_ref().unwrap()
        }
    }

    #[test]
    fn master_main_loop() {
        let data_in = b"asd";
        let mut serial = SerialInterface::new(data_in);
        let mut registers: HashMap<Parameter, Value> = HashMap::new();

        let mut master_proto = MasterEnum::new();
    }
}
