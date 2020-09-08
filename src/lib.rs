//! Sans-IO implementation of the ANSI X3.28 serial line protocol
//!
//! X3.28 is an old fieldbus protocol, commonly used on top of a RS-422 bus.
//! The bus settings should be 9600 baud, 7 bit char, no flow control, even parity, 1 stop bit (7E1).
//! Since this crate doesn't provide IO at all, feel free to use whatever transport you want.

pub mod master;
pub mod slave;

pub use master::Master;
pub use slave::Slave;
pub use types::{Address, Parameter, Value};

mod buffer;
mod nom_parser;
mod types;

use std::error::Error as StdError;
use std::fmt::{self, Formatter};

#[derive(Debug, PartialEq)]
pub enum X328Error {
    InvalidAddress,
    InvalidParameter,
    IOError,
    OtherError,
    InvalidDataReceived,
    WriteNAK,
}

impl StdError for X328Error {}

impl fmt::Display for X328Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<std::io::Error> for X328Error {
    fn from(_: std::io::Error) -> Self {
        X328Error::IOError
    }
}

/// Calculates the BCC checksum according to the X3.28 spec.
pub(crate) fn bcc(data: &[u8]) -> u8 {
    let mut checksum: u8 = 0;
    for byte in data {
        checksum ^= *byte;
    }
    if checksum < 0x20 {
        checksum += 0x20;
    }
    checksum
}
