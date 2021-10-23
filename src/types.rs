use thiserror::Error;

use arrayvec::ArrayVec;
use std::convert::{TryFrom, TryInto};
use std::ops::{Deref, RangeInclusive};

/// Error type for this module
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The value isn't a valid X3.28 node address.
    #[error("Invalid address")]
    InvalidAddress,
    /// The value isn't a valid X3.28 parameter.
    #[error("Invalid parameter")]
    InvalidParameter,
    /// The value isn't a valid X3.28 value.
    #[error("Invalid value")]
    InvalidValue,
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
    /// Create a new address, checking that the address is in \[0, 99\].
    /// # Errors
    /// Returns [`Error::InvalidAddress`] if `address` is out of range.
    pub fn new(address: impl TryInto<u8>) -> Result<Self, Error> {
        let address = address
            .try_into()
            .map_err(|_| Error::InvalidAddress)
            .and_then(|addr| {
                if (0u8..100).contains(&addr) {
                    Ok(addr)
                } else {
                    Err(Error::InvalidAddress)
                }
            })?;
        Ok(Self(address))
    }

    pub(crate) const fn to_bytes(self) -> [u8; 4] {
        let mut buf = [0; 4];
        buf[0] = 0x30 + self.0 / 10;
        buf[1] = buf[0];
        buf[2] = 0x30 + self.0 % 10;
        buf[3] = buf[2];
        buf
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

/// Trait to convert `T: TryInto<u8>` into an [`Address`].
pub trait IntoAddress {
    /// Convert self to an Address.
    /// # Errors
    /// Returns `Error:InvalidAddress` if self isn't a valid address.
    fn into_address(self) -> Result<Address, Error>;
}

impl IntoAddress for Address {
    fn into_address(self) -> Result<Address, Error> {
        Ok(self)
    }
}

impl<T> IntoAddress for T
where
    T: TryInto<u8>,
{
    fn into_address(self) -> Result<Address, Error> {
        Address::new(self)
    }
}

impl TryFrom<usize> for Address {
    type Error = Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod address_tests {
    use super::Address;

    #[test]
    fn test_valid_addresses() {
        for n in 0..=99 {
            let a = Address::new(n).unwrap();
            assert_eq!(*a, n);
            let bytes = a.to_bytes();
            assert_eq!(bytes[0], bytes[1]);
            assert_eq!(bytes[2], bytes[3]);
        }
    }

    #[test]
    fn test_address() {
        let a05 = Address::new(5).unwrap();
        assert_eq!(&a05.to_bytes(), b"0055");

        assert!(Address::new(100).is_err());
        assert!(Address::new(-1).is_err());
    }
}

/// `Parameter` is a range-checked \[0, 9999\] integer, representing a register
/// in a node.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
#[repr(transparent)]
pub struct Parameter(i16);

impl Parameter {
    /// Create a new `Parameter`, checking that the given value
    /// is in the range [0, 9999].
    /// # Errors
    /// Returns [`Error::InvalidParameter`] if `parameter` is out of range.
    pub fn new(parameter: impl TryInto<i16>) -> Result<Self, Error> {
        let parameter = parameter.try_into().map_err(|_| Error::InvalidParameter)?;
        if (0..=9999).contains(&parameter) {
            Ok(Self(parameter))
        } else {
            Err(Error::InvalidParameter)
        }
    }

    pub(crate) fn to_bytes(self) -> [u8; 4] {
        let mut buf = [0; 4];
        let mut x = self.0;
        for c in buf.iter_mut().rev() {
            *c = 0x30 + (x % 10) as u8;
            x /= 10;
        }
        buf
    }

    /// Returns the next higher numbered parameter, or None if the current value is at max.
    pub fn next(self) -> Option<Self> {
        if self.0 < 9999 {
            Some(Self(self.0 + 1))
        } else {
            None
        }
    }

    /// Returns the next lowered numbered parameter, or None if the current value is zero.
    pub fn prev(self) -> Option<Self> {
        if self.0 > 0 {
            Some(Self(self.0 - 1))
        } else {
            None
        }
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

/// Trait to convert `T: TryInto<i16>` into a [`Parameter`].
pub trait IntoParameter {
    /// Convert `self` to `Parameter`.
    /// # Errors
    /// Returns [`Error::InvalidParameter`] if `self` can't be converted.
    fn into_parameter(self) -> Result<Parameter, Error>;
}

impl IntoParameter for Parameter {
    fn into_parameter(self) -> Result<Parameter, Error> {
        Ok(self)
    }
}

impl<T> IntoParameter for T
where
    T: TryInto<i16>,
{
    fn into_parameter(self) -> Result<Parameter, Error> {
        Parameter::new(self)
    }
}

impl TryFrom<usize> for Parameter {
    type Error = Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod parameter_tests {
    use super::Parameter;

    #[test]
    fn test_parameter() {
        assert_eq!(Parameter::new(10).unwrap(), Parameter(10));

        let p10 = Parameter::new(10).unwrap();
        assert_eq!(p10, 10); // usize comparison

        let str = &p10.to_bytes();
        assert_eq!(str, b"0010");
    }

    #[test]
    fn test_parameter_next_prev() {
        let p0 = Parameter(0);
        assert_eq!(p0.prev(), None);
        assert_eq!(p0.next(), Some(Parameter(1)));
        let p10 = Parameter(10);
        assert_eq!(p10.prev(), Some(Parameter(9)));
        assert_eq!(p10.next(), Some(Parameter(11)));
        let p9999 = Parameter(9999);
        assert_eq!(p9999.prev(), Some(Parameter(9998)));
        assert_eq!(p9999.next(), None);
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

/// Value represents an integer that can be sent over the X3.28 protocol.
///
/// It is range limited to [-99999, 999999], since the on-wire representation
/// is limited to six ascii characters.
#[derive(Debug, Copy, Clone)]
pub struct Value(i32, ValueFormat);

pub(crate) type ValueBytes = ArrayVec<u8, 6>;

const VAL_RANGE: RangeInclusive<i32> = -99_999..=999_999;
const VAL_MIN_NORM: i32 = -9999;

impl Value {
    /// Create a new `Value`, checking that the given `value` can be represented
    /// in the on-wire format.
    /// # Errors
    /// Returns [`Error::InvalidValue`] if `value` is out of range.
    pub fn new(value: impl TryInto<i32>) -> Result<Self, Error> {
        let value: i32 = value.try_into().map_err(|_| Error::InvalidValue)?;
        if !VAL_RANGE.contains(&value) {
            return Err(Error::InvalidValue);
        }
        let fmt = {
            if value < VAL_MIN_NORM {
                ValueFormat::Wide
            } else {
                ValueFormat::Normal
            }
        };
        Ok(Self(value, fmt))
    }

    /// Create a new Value, specifying the on-wire format mode, normal or wide.
    pub fn new_fmt(value: i32, format: ValueFormat) -> Result<Self, Error> {
        if !VAL_RANGE.contains(&value) || format == ValueFormat::Normal && value < VAL_MIN_NORM {
            return Err(Error::InvalidValue);
        }
        Ok(Self(value, format))
    }

    /// Returns the contained value as u16 if it can be converted without truncation.
    pub fn try_into_u16(self) -> Option<u16> {
        u16::try_from(self.0).ok()
    }

    /// Format the value into the on-wire representation.
    pub(crate) fn to_bytes(self) -> ValueBytes {
        let mut val = self.0.abs();
        let mut buf = ValueBytes::new();
        loop {
            buf.push(b'0' + (val % 10) as u8); // push panics on overflow
            val /= 10;
            if val == 0 && (self.1 == ValueFormat::Normal || buf.len() == 5) {
                break;
            }
        }
        if self.0.is_negative() {
            buf.push(b'-');
        } else if !buf.is_full() {
            buf.push(b'+');
        }
        buf.reverse();
        buf
    }
}

/// Trait to convert `T: Into<i32>` into a [`Value`].
pub trait IntoValue {
    /// Try to convert self to a `Value`
    /// # Errors
    /// Returns [`Error::InvalidValue`] if self isn't a valid address.
    fn into_value(self) -> Result<Value, Error>;
}

impl IntoValue for Value {
    fn into_value(self) -> Result<Value, Error> {
        Ok(self)
    }
}

impl<T> IntoValue for T
where
    T: TryInto<i32>,
{
    fn into_value(self) -> Result<Value, Error> {
        Value::new(self)
    }
}

impl TryFrom<i32> for Value {
    type Error = Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<u16> for Value {
    fn from(val: u16) -> Self {
        Self(val.into(), ValueFormat::Normal)
    }
}

impl From<i16> for Value {
    fn from(val: i16) -> Self {
        let val = val.into();
        let fmt = if val < VAL_MIN_NORM {
            ValueFormat::Wide
        } else {
            ValueFormat::Normal
        };
        Self(val, fmt)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<i32> for Value {
    fn eq(&self, other: &i32) -> bool {
        self.0 == *other
    }
}

impl Deref for Value {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
