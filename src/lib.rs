use nom::lib::std::fmt::Formatter;
use std::error::Error as StdError;
use std::fmt;

mod buffer;
pub mod master;
mod nom_parser;
pub mod slave;
mod types;

pub(crate) use types::ParameterOffset;
pub use types::{Address, Parameter, Value};

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
