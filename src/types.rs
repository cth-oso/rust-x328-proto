use crate::X328Error;
use std::cmp::Ordering;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;

pub type Value = i32;

#[derive(PartialEq, Eq, Debug, Copy, Clone, Hash)]
pub struct Address(u8);

impl Address {
    pub fn new(address: u8) -> Result<Address, X328Error> {
        if address <= 99 {
            Ok(Address(address))
        } else {
            Err(X328Error::InvalidAddress)
        }
    }

    pub fn new_unchecked(address: u8) -> Address {
        Address::new(address).expect("Address out of range")
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        use std::io::Write;
        let mut buf = vec![0; 4];
        write!(&mut buf[1..], "{:02}", self.0).expect("Address formatting failed");
        buf[0] = buf[1];
        buf[3] = buf[2];
        buf
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
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

impl TryFrom<usize> for Address {
    type Error = X328Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Address::new(value.try_into().map_err(|_| X328Error::InvalidAddress)?)
    }
}

impl FromStr for Address {
    type Err = X328Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 2 {
            Err(X328Error::InvalidAddress)
        } else {
            Address::new(s.parse().map_err(|_e| X328Error::InvalidAddress)?)
        }
    }
}

impl ToString for Address {
    fn to_string(&self) -> String {
        String::from_utf8(self.to_bytes()).expect("Failed to construct address string")
    }
}

#[derive(PartialEq, Eq, Debug, Copy, Clone, Hash)]
pub struct Parameter(i16);
pub(crate) type ParameterOffset = i16;

impl Parameter {
    /// Create a new Parameter, checking that the given value
    /// is in the range [0..9999].
    pub fn new(parameter: i16) -> Result<Parameter, X328Error> {
        if (0 <= parameter) && (parameter <= 9999) {
            Ok(Parameter(parameter))
        } else {
            Err(X328Error::InvalidParameter)
        }
    }
    /// Panics if parameter is outside of the range 0..9999
    pub fn new_unchecked(parameter: i16) -> Parameter {
        Parameter::new(parameter).expect("Parameter out of range")
    }

    pub(crate) fn checked_add(&self, offset: ParameterOffset) -> Result<Parameter, X328Error> {
        Parameter::new(
            self.0
                .checked_add(offset)
                .ok_or(X328Error::InvalidParameter)?,
        )
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        use std::io::Write;
        let mut buf = vec![0; 4];
        write!(buf.as_mut_slice(), "{:04}", self.0).expect("Parameter format failed");
        buf
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

impl PartialEq<usize> for Parameter {
    fn eq(&self, other: &usize) -> bool {
        self.0 as usize == *other
    }
}

impl PartialOrd<usize> for Parameter {
    fn partial_cmp(&self, other: &usize) -> Option<Ordering> {
        if *other > 9999 {
            Some(Ordering::Less)
        } else {
            Some(self.0.cmp(&(*other as i16)))
        }
    }
}

impl Into<usize> for Parameter {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl Into<i16> for Parameter {
    fn into(self) -> i16 {
        self.0 as i16
    }
}

impl TryFrom<usize> for Parameter {
    type Error = X328Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Parameter::new(value.try_into().map_err(|_| X328Error::InvalidParameter)?)
    }
}

impl FromStr for Parameter {
    type Err = X328Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 4 {
            Err(X328Error::InvalidParameter)
        } else {
            Parameter::new(s.parse().map_err(|_e| X328Error::InvalidParameter)?)
        }
    }
}

impl ToString for Parameter {
    fn to_string(&self) -> String {
        String::from_utf8(self.to_bytes()).expect("Parameter to string failed")
    }
}

#[cfg(test)]
mod tests {
    use super::{Address, Parameter};

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
        assert_eq!(p10.checked_add(10), Ok(Parameter(20)));
        assert_eq!(p10.checked_add(-10), Ok(Parameter(0)));
        assert!(p10.checked_add(-20).is_err());

        assert!(Parameter(9999).checked_add(1).is_err());

        let str = p10.to_string();
        assert_eq!(str, "0010");

        assert_eq!("0010".parse(), Ok(p10));
        assert_eq!("0100".parse(), Ok(Parameter(100)));
        assert!("10".parse::<Parameter>().is_err());
        assert!("-100".parse::<Parameter>().is_err());
        assert!("00010".parse::<Parameter>().is_err());
    }

    #[test]
    fn test_parameter_ordering() {
        let p9999 = Parameter(9999);
        assert_eq!(p9999, 9999);
        assert!(p9999 < 10_000);
        assert!(p9999 > 9998);
    }
}
