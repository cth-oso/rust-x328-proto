/*!
The [`Scanner`] is used to reconstruct X3.28 bus events from byte streams generated by the bus
controller and the nodes. Useful for sniffing a X3.28 bus, or transparently splitting it into segments.
*/

use crate::master::{self, Master, SendData};
use crate::nom_parser::node::{scan_command, CommandToken};
use crate::{addr, param, value, Address, Parameter, Value};

/// Decode data from both the master and node channels, and turn it into X3.28 messages
#[derive(Debug)]
pub struct Scanner {
    expect: Expect,
    read_again: Option<(Address, Parameter)>,
}

#[derive(Debug, PartialEq)]
enum Expect {
    Command,
    ReadResponse(Address, Parameter),
    WriteResponse,
}

/// Events generated by transmissions from the bus controller.
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerEvent {
    /// Parameter read request
    Read(Address, Parameter),
    /// Parameter write request
    Write(Address, Parameter, Value),
    /// The bus controller issued a new request without receiving a response to the previous one.
    NodeTimeout,
}

/// Events generated by transmission from a bus node.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// Write request response
    Write(Result<(), master::Error>),
    /// Read request response
    Read(Result<Value, master::Error>),
    /// Data was received from a node without a corresponding bus controller request
    UnexpectedTransmission,
}

/// This enum can contain either a node event or a controller event.
pub enum Event {
    /// Event generated by data on the controller tx channel
    Ctrl(ControllerEvent),
    /// Event generated by data on the node tx channel
    Node(NodeEvent),
}

impl From<ControllerEvent> for Event {
    fn from(value: ControllerEvent) -> Self {
        Self::Ctrl(value)
    }
}

impl From<NodeEvent> for Event {
    fn from(value: NodeEvent) -> Self {
        Self::Node(value)
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    /// Create a new scanner instance.
    pub fn new() -> Self {
        Self {
            expect: Expect::Command,
            read_again: None,
        }
    }

    /// Parse data from the bus controller. The return value is the number of bytes consumed
    /// to generate the returned event. `&data[consumed..]` should be passed in the next call,
    /// together with any newly received data.
    ///
    /// Invalid leading data will be consumed, but None will be returned instead of an event.
    pub fn recv_from_ctrl(&mut self, data: &[u8]) -> (usize, Option<ControllerEvent>) {
        let read_again = self.read_again.take();

        if self.expect != Expect::Command {
            self.expect = Expect::Command;
            return (0, Some(ControllerEvent::NodeTimeout));
        }

        let (consumed, token) = scan_command(data);
        let event = match token {
            CommandToken::WriteParameter(a, p, v) => {
                self.expect = Expect::WriteResponse;
                Some(ControllerEvent::Write(a, p, v))
            }
            CommandToken::ReadParameter(a, p) => {
                self.expect = Expect::ReadResponse(a, p);
                self.read_again = Some((a, p));
                Some(ControllerEvent::Read(a, p))
            }
            CommandToken::ReadPrevious | CommandToken::ReadAgain | CommandToken::ReadNext
                if read_again.is_some() =>
            {
                let (ra, rp) = read_again.unwrap();
                match token {
                    CommandToken::ReadPrevious => rp.prev(),
                    CommandToken::ReadAgain => Some(rp),
                    CommandToken::ReadNext => rp.next(),
                    _ => unreachable!(),
                }
                .map(|p| {
                    self.expect = Expect::ReadResponse(ra, p);
                    self.read_again = Some((ra, p));
                    ControllerEvent::Read(ra, p)
                })
            }
            CommandToken::ReadPrevious | CommandToken::ReadAgain | CommandToken::ReadNext => {
                None // The controller issued a read again command without a preceding read command
            }
            CommandToken::InvalidPayload(_) => None,
            CommandToken::NeedData => None,
        };
        return (consumed, event);
    }

    /// Parse data from the bus nodes. The return value is the number of bytes consumed
    /// to generate the returned event. `&data[consumed..]` should be passed in the next call,
    /// together with any newly received data.
    pub fn recv_from_node(&mut self, data: &[u8]) -> (usize, Option<NodeEvent>) {
        let mut ctrl = Master::new();
        let len = data.len();
        let mut data = data.iter();
        match &self.expect {
            Expect::Command => return (len, NodeEvent::UnexpectedTransmission.into()),
            Expect::ReadResponse(addr, param) => {
                let mut send = ctrl.read_parameter(*addr, *param);
                let recv = send.data_sent();
                while let Some(byte) = data.next() {
                    if let Some(resp) = recv.receive_data([*byte].as_slice()) {
                        self.expect = Expect::Command;
                        return (len - data.as_slice().len(), NodeEvent::Read(resp).into());
                    }
                }
            }
            Expect::WriteResponse => {
                let mut send = ctrl.write_parameter(addr(1), param(1), value(1));
                let recv = send.data_sent();
                while let Some(byte) = data.next() {
                    if let Some(resp) = recv.receive_data([*byte].as_slice()) {
                        self.expect = Expect::Command;
                        return (len - data.as_slice().len(), NodeEvent::Write(resp).into());
                    }
                }
            }
        }

        return (0, None); // the caller needs to call us with the old data as well as the new
    }
}
