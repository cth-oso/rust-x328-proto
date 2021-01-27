use snafu::{ensure, Backtrace, OptionExt, Snafu};

use arrayvec::ArrayVec;
use std::convert::{TryFrom, TryInto};
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    #[snafu(display("Invalid address {}", address))]
    InvalidAddress {
        address: String,
        backtrace: Backtrace,
    },
    #[snafu(display("Invalid parameter {}", parameter))]
    InvalidParameter {
        parameter: String,
        backtrace: Backtrace,
    },
    #[snafu(display("Invalid value {}", value))]
    InvalidValue { value: String, backtrace: Backtrace },
}

fn invalid_address<T: ToString>(address: T) -> InvalidAddress<String> {
    InvalidAddress {
        address: address.to_string(),
    }
}

fn invalid_parameter<T: ToString>(parameter: T) -> InvalidParameter<String> {
    InvalidParameter {
        parameter: parameter.to_string(),
    }
}

/// Address is a range-checked [0, 99] integer, representing a node address.
///
/// ## Example
/// ```
/// use x328_proto::Address;
/// use std::convert::TryInto;
/// let addr = Address::new(10).unwrap();
/// let addr: Address = 10.try_into().unwrap();
/// ```
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
#[repr(transparent)]
pub struct Address(u8);

impl Address {
    /// Create a new address, checking that the address is in [0,99].
    pub fn new(address: u8) -> Result<Address, Error> {
        ensure!(address <= 99, invalid_address(address));
        Ok(Address(address))
    }

    pub(crate) fn to_bytes(&self) -> [u8; 4] {
        let mut buf = [0; 4];
        buf[0] = 0x30 + self.0 / 10;
        buf[1] = buf[0];
        buf[2] = 0x30 + self.0 % 10;
        buf[3] = buf[2];
        buf
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

impl Deref for Address {
    type Target = u8;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<usize> for Address {
    fn eq(&self, other: &usize) -> bool {
        self.0 as usize == *other
    }
}

pub trait IntoAddress: TryInto<Address> {
    fn into_address(self) -> Result<Address, Error>;
}

impl IntoAddress for Address {
    fn into_address(self) -> Result<Address, Error> {
        Ok(self)
    }
}

impl<T> IntoAddress for T
where
    T: TryInto<Address> + ToString + Clone,
{
    fn into_address(self) -> Result<Address, Error> {
        let cpy = self.clone();
        self.try_into().ok().with_context(|| invalid_address(cpy))
    }
}

impl TryFrom<usize> for Address {
    type Error = Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        ensure!(value <= 99, invalid_address(value));
        Address::new(value as u8)
    }
}

impl FromStr for Address {
    type Err = Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ensure!(s.len() == 2, invalid_address(s));
        Address::new(s.parse().ok().with_context(|| invalid_address(s))?)
    }
}

/// Parameter is a range-checked [0,9999] integer, representing a node parameter.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
#[repr(transparent)]
pub struct Parameter(i16);
pub(crate) type ParameterOffset = i16;

impl Parameter {
    /// Create a new Parameter, checking that the given value
    /// is in the range [0, 9999].
    pub fn new(parameter: i16) -> Result<Parameter, Error> {
        ensure!(
            (0..=9999).contains(&parameter),
            invalid_parameter(parameter)
        );
        Ok(Parameter(parameter))
    }

    pub(crate) fn checked_add(&self, offset: ParameterOffset) -> Result<Parameter, Error> {
        Parameter::new(
            self.0
                .checked_add(offset)
                .with_context(|| invalid_parameter("Checked add failed"))?,
        )
    }

    pub(crate) fn to_bytes(&self) -> [u8; 4] {
        let mut buf = [0; 4];
        let mut x = self.0;
        for c in buf.iter_mut().rev() {
            *c = 0x30 + (x % 10) as u8;
            x /= 10;
        }
        buf
    }
}

impl Deref for Parameter {
    type Target = i16;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<usize> for Parameter {
    fn eq(&self, other: &usize) -> bool {
        self.0 as usize == *other
    }
}

pub trait IntoParameter {
    fn into_parameter(self) -> Result<Parameter, Error>;
}

impl IntoParameter for Parameter {
    fn into_parameter(self) -> Result<Parameter, Error> {
        Ok(self)
    }
}

impl<T> IntoParameter for T
where
    T: TryInto<i16> + ToString + Clone,
{
    fn into_parameter(self) -> Result<Parameter, Error> {
        let e = self.clone();
        Parameter::new(self.try_into().ok().with_context(|| invalid_parameter(e))?)
    }
}

impl TryFrom<usize> for Parameter {
    type Error = Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        ensure!(value <= 9999, invalid_parameter(value));
        Parameter::new(value as i16)
    }
}

impl FromStr for Parameter {
    type Err = Error;

    /// This is meant to be used for parsing the on-wire format
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ensure!(s.len() == 4, invalid_parameter(s));
        Parameter::new(s.parse().ok().with_context(|| invalid_parameter(s))?)
    }
}

#[cfg(test)]
mod tests {
    use super::{Address, Parameter};

    macro_rules! assert_ok {
        ($res:expr, $ok:expr) => {
            assert_eq!($res.unwrap(), $ok)
        };
    }

    #[test]
    fn test_address() {
        let a87 = Address::new(87).unwrap();
        assert_eq!(a87, 87);

        let bytes = &a87.to_bytes();
        assert_eq!(bytes, b"8877");

        let a05 = Address::new(5).unwrap();
        assert_eq!(&a05.to_bytes(), b"0055");

        assert_eq!("05".parse::<Address>().unwrap(), Address(5));
        assert_eq!("13".parse::<Address>().unwrap(), 13);
        assert!("1".parse::<Address>().is_err());
        assert!("100".parse::<Address>().is_err());
    }

    #[test]
    fn test_parameter() {
        assert_eq!(Parameter::new(10).unwrap(), Parameter(10));

        let p10 = Parameter::new(10).unwrap();
        assert_eq!(p10, 10); // usize comparison
        assert_ok!(p10.checked_add(10), Parameter(20));
        assert_ok!(p10.checked_add(-10), Parameter(0));
        assert!(p10.checked_add(-20).is_err());

        assert!(Parameter(9999).checked_add(1).is_err());
        assert!(Parameter(9999).checked_add(32000).is_err());

        let str = &p10.to_bytes();
        assert_eq!(str, b"0010");

        assert_ok!("0010".parse::<Parameter>(), p10);
        assert_ok!("0100".parse::<Parameter>(), Parameter(100));
        assert!("10".parse::<Parameter>().is_err());
        assert!("-100".parse::<Parameter>().is_err());
        assert!("00010".parse::<Parameter>().is_err());
    }

    #[test]
    fn test_parameter_ordering() {
        let p9999 = Parameter(9999);
        assert_eq!(p9999, 9999);
        assert!(*p9999 < 10_000);
        assert!(*p9999 > 9998);
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ValueFormat {
    Wide,
    Normal,
}

#[derive(Debug, Copy, Clone)]
pub struct Value(i32, ValueFormat);

pub(crate) type ValueBytes = ArrayVec<[u8; 6]>;

impl Value {
    pub const fn new(value: i32) -> Option<Value> {
        if value < -99999 || value > 99999 {
            return None;
        }
        let fmt = {
            if value < -9999 {
                ValueFormat::Wide
            } else {
                ValueFormat::Normal
            }
        };
        Some(Value(value, fmt))
    }

    pub const fn new_fmt(value: i32, fmt: ValueFormat) -> Option<Value> {
        let (min, max) = match fmt {
            ValueFormat::Wide => (-99999, 99999),
            ValueFormat::Normal => (-9999, 99999),
        };
        if value < min || value > max {
            return None;
        }
        Some(Value(value, fmt))
    }

    pub fn try_u16(&self) -> Option<u16> {
        self.0.try_into().ok()
    }

    pub(crate) fn to_bytes(&self) -> ValueBytes {
        match self.1 {
            ValueFormat::Wide => {
                let mut buf = [0_u8; 6];
                let mut val = self.0;
                buf[0] = match val >= 0 {
                    true => b'+',
                    false => b'-',
                };
                for b in (&mut buf[1..]).iter_mut().rev() {
                    *b = b'0' + (val % 10) as u8;
                    val /= 10;
                }
                debug_assert_eq!(val, 0, "Value didn't fit in Wide format: {}.", self.0);
                buf.into()
            }
            ValueFormat::Normal => {
                let mut buf = ValueBytes::new();
                let mut val = self.0;
                let positive = val >= 0;
                loop {
                    buf.push(b'0' + (val % 10) as u8);
                    val /= 10;
                    if val == 0 {
                        break;
                    }
                }
                if buf.len() < 5 {
                    buf.push(match positive {
                        true => b'+',
                        false => b'-',
                    });
                } else {
                    debug_assert!(
                        positive && buf.len() == 5,
                        "Value to large to transmit in Normal X3.28 format: {}.",
                        self.0
                    );
                }
                buf.reverse();
                buf
            }
        }
    }
}

pub trait IntoValue {
    fn into_value(self) -> Option<Value>;
}

impl IntoValue for Value {
    fn into_value(self) -> Option<Value> {
        Some(self)
    }
}

impl<T> IntoValue for T
where
    T: Into<i32>,
{
    fn into_value(self) -> Option<Value> {
        let value = self.into();
        if !(-99999..=99999).contains(&value) {
            return None;
        }
        let fmt = {
            if value < -9999 {
                ValueFormat::Wide
            } else {
                ValueFormat::Normal
            }
        };
        Some(Value(value, fmt))
    }
}

impl From<u16> for Value {
    fn from(val: u16) -> Self {
        Value(val as i32, ValueFormat::Normal)
    }
}

impl From<i16> for Value {
    fn from(val: i16) -> Self {
        let fmt = if val < -9999 {
            ValueFormat::Wide
        } else {
            ValueFormat::Normal
        };
        Value(val as i32, fmt)
    }
}

impl FromStr for Value {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let fmt = match s.len() {
            1..=5 => ValueFormat::Normal,
            6 => ValueFormat::Wide,
            _ => {
                return InvalidValue {
                    value: s.to_owned(),
                }
                .fail()
            }
        };
        Ok(Value(
            s.parse().ok().with_context(|| InvalidValue {
                value: s.to_owned(),
            })?,
            fmt,
        ))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Deref for Value {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
