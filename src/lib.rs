use nom::lib::std::fmt::Formatter;
use nom::lib::std::str::FromStr;
use std::error::Error as StdError;
use std::fmt;

mod buffer;
pub mod master;
mod nom_parser;
pub mod slave;

pub type Value = i32;

#[derive(PartialEq, Eq, Debug, Copy, Clone, Hash)]
pub struct Address(u8);

impl Address {
    pub fn new(address: u8) -> Option<Address> {
        if address <= 99 {
            Some(Address(address))
        } else {
            None
        }
    }

    pub fn new_unchecked(address: u8) -> Address {
        Address::new(address).expect("Address out of range")
    }
}

impl PartialEq<usize> for Address {
    fn eq(&self, other: &usize) -> bool {
        self.0 as usize == *other
    }
}

impl Into<usize> for Address {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl FromStr for Address {
    type Err = X328Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 2 {
            Err(X328Error::InvalidAddress)
        } else {
            Address::new(s.parse().map_err(|e| X328Error::InvalidAddress)?)
                .ok_or(X328Error::InvalidAddress)
        }
    }
}

impl ToString for Address {
    fn to_string(&self) -> String {
        let addr_str = format!("{:02}", self.0);
        let x = addr_str.as_bytes();
        String::from_utf8(vec![x[0], x[0], x[1], x[1]]).expect("Failed to construct address string")
    }
}

#[derive(PartialEq, Eq, Debug, Copy, Clone, Hash)]
pub struct Parameter(i16);
pub(crate) type ParameterOffset = i16;

impl Parameter {
    /// Create a new Parameter, checking that the given value
    /// is in the range [0..9999].
    pub fn new(parameter: i16) -> Option<Parameter> {
        if (0 <= parameter) && (parameter <= 9999) {
            Some(Parameter(parameter))
        } else {
            None
        }
    }
    /// Panics if parameter is outside of the range 0..9999
    pub fn new_unchecked(parameter: i16) -> Parameter {
        Parameter::new(parameter).expect("Parameter out of range")
    }

    pub(crate) fn checked_add(&self, offset: ParameterOffset) -> Option<Parameter> {
        Parameter::new(self.0.checked_add(offset)?)
    }
}

impl PartialEq<usize> for Parameter {
    fn eq(&self, other: &usize) -> bool {
        self.0 as usize == *other
    }
}

impl Into<usize> for Parameter {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl FromStr for Parameter {
    type Err = X328Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 4 {
            Err(X328Error::InvalidParameter)
        } else {
            Parameter::new(s.parse().map_err(|e| X328Error::InvalidParameter)?)
                .ok_or(X328Error::InvalidParameter)
        }
    }
}

impl ToString for Parameter {
    fn to_string(&self) -> String {
        format!("{:04}", self.0)
    }
}

#[derive(Debug, PartialEq)]
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

#[cfg(test)]
mod tests {
    use crate::{Address, Parameter};

    #[test]
    fn test_address() {
        let a87 = Address::new(87).unwrap();
        assert_eq!(a87, 87);

        let str = a87.to_string();
        assert_eq!(str, "8877");

        let a05 = Address::new_unchecked(5);
        assert_eq!(a05.to_string(), "0055");

        assert_eq!("05".parse::<Address>().unwrap(), Address(5));
        assert_eq!("13".parse::<Address>().unwrap(), 13);
        assert!("1".parse::<Address>().is_err());
        assert!("100".parse::<Address>().is_err());
    }

    #[test]
    fn test_parameter() {
        assert_eq!(Parameter::new(10).unwrap(), Parameter(10));

        let p10 = Parameter::new_unchecked(10);
        assert_eq!(p10, 10); // usize comparison
        assert_eq!(p10.checked_add(10), Some(Parameter(20)));
        assert_eq!(p10.checked_add(-10), Some(Parameter(0)));
        assert_eq!(p10.checked_add(-20), None);

        assert_eq!(Parameter(9999).checked_add(1), None);

        let str = p10.to_string();
        assert_eq!(str, "0010");

        assert_eq!("0010".parse(), Ok(p10));
        assert_eq!("0100".parse(), Ok(Parameter(100)));
        assert!("10".parse::<Parameter>().is_err());
        assert!("-100".parse::<Parameter>().is_err());
        assert!("00010".parse::<Parameter>().is_err());
    }
}
