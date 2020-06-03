use nom::lib::std::fmt::Formatter;
use std::error::{Error as StdError};
use std::fmt;

mod buffer;
pub mod master;
mod nom_parser;
pub mod slave;

pub type Address = u8;
pub type Parameter = u16;
pub type Value = i32;

#[derive(Debug)]
pub enum X328Error {
    InvalidAddress,
    InvalidParameter,
    IOError,
    OtherError,
}

impl StdError for X328Error {}

impl fmt::Display for X328Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        use X328Error::*;
        match self {
            InvalidAddress => write!(f, "Invalid address"),
            InvalidParameter => write!(f, "Invalid parameter"),
            _ => write!(f, "Haha"),
        }
    }
}

impl From<std::io::Error> for X328Error {
    fn from(_: std::io::Error) -> Self {
        X328Error::IOError
    }
}
