use ascii::AsciiChar::{self, BackSpace, ACK, ENQ, EOT, ETX, NAK, SOX};

use nom::error::ParseError;
use nom::Err::Incomplete;

use nom::branch::alt;
use nom::bytes::streaming::{take, take_while, take_while_m_n};
use nom::sequence::{pair, preceded, terminated, tuple};
use nom::IResult;

use nom::combinator::{map, map_res, peek, recognize, value};

use crate::slave::{Address, Parameter, Value};

use AddressToken::{Invalid, Valid};
use CommandToken::{NeedData, ReadAgain, Reset, SendNAK, WriteParameter};

type Buf = str;

#[derive(PartialEq, Debug)]
pub enum AddressToken {
    Valid(Address),
    Invalid,
}

#[derive(PartialEq, Debug)]
pub enum CommandToken {
    Reset(AddressToken),
    ReadParameter(Parameter),
    WriteParameter(Parameter, Value),
    ReadAgain(i32),
    SendNAK,
    NeedData,
}

#[derive(Clone)]
pub enum ResponseToken {
    WriteOk,
    WriteFailed,
    InvalidParameter,
    ReadOK { parameter: Parameter, value: Value },
    ReadFailed,
    NeedData,
}

pub fn parse_command(buf: &Buf) -> (usize, CommandToken) {
    match alt((
        slave_reset,
        write_command,
        read_command,
        read_again,
        read_next,
        read_previous,
        read_until_eot,
    ))(buf)
    {
        Ok((remaining, token)) => (buf.len() - remaining.len(), token),
        Err(Incomplete(_)) => (0, NeedData),
        Err(_) => panic!("Wut??"),
    }
}

pub fn parse_reponse(buf: &Buf) -> (usize, ResponseToken) {
    match alt((
        value(ResponseToken::WriteOk, ascii_char(ACK)),
        value(ResponseToken::WriteFailed, ascii_char(NAK)),
        value(ResponseToken::InvalidParameter, ascii_char(EOT)),
        read_response,
    ))(buf)
    {
        Ok((remaining, token)) => (buf.len() - remaining.len(), token),
        Err(Incomplete(_)) => (0, ResponseToken::NeedData),
        Err(_) => panic!("FIXME bad response handling"), // FIXME
    }
}

fn read_response(buf: &Buf) -> IResult<&Buf, ResponseToken> {
    match param_data_etx(buf) {
        Ok((buf, (parameter, value, true))) => {
            Ok((buf, ResponseToken::ReadOK { parameter, value }))
        }
        Ok((buf, (_parameter, _value, false))) => Ok((buf, ResponseToken::ReadFailed)),
        Err(Incomplete(x)) => Err(Incomplete(x)),
        Err(_) => Ok((buf, ResponseToken::ReadFailed)),
    }
}
fn read_until_eot(buf: &Buf) -> IResult<&Buf, CommandToken> {
    let (buf, _) = take_while(|c| c != EOT.as_char())(buf)?;
    slave_reset(buf)
}

fn slave_reset(buf: &Buf) -> IResult<&Buf, CommandToken> {
    map(preceded(ascii_char(EOT), address), Reset)(buf)
}

fn read_next(buf: &Buf) -> IResult<&Buf, CommandToken> {
    let (buf, _) = ascii_char(ACK)(buf)?;
    Ok((buf, ReadAgain(1)))
}

fn read_again(buf: &Buf) -> IResult<&Buf, CommandToken> {
    let (buf, _) = ascii_char(NAK)(buf)?;
    Ok((buf, ReadAgain(0)))
}

fn read_previous(buf: &Buf) -> IResult<&Buf, CommandToken> {
    let (buf, _) = ascii_char(BackSpace)(buf)?;
    Ok((buf, ReadAgain(-1)))
}

fn address<'a, E: ParseError<&'a Buf>>(buf: &'a Buf) -> IResult<&'a Buf, AddressToken, E> {
    let (buf, addr) = take(4usize)(buf)?;
    if addr[0..1] == addr[1..2] && addr[2..3] == addr[3..] {
        if let Ok(addr_int) = addr[1..3].parse::<Address>() {
            return Ok((buf, Valid(addr_int)));
        }
    }
    Ok((buf, Invalid))
}

fn parameter<'a, E: ParseError<&'a Buf>>(buf: &'a Buf) -> IResult<&'a Buf, Parameter, E> {
    map_int(take(4usize))(buf)
}

fn x328_value<'a, E: ParseError<&'a Buf>>(buf: &'a Buf) -> IResult<&'a Buf, Value, E> {
    map_int(take_while_m_n(1, 6, |c| c != ETX.as_char()))(buf)
}

fn read_command<'a, E: ParseError<&'a Buf>>(buf: &'a Buf) -> IResult<&'a Buf, CommandToken, E> {
    map(
        terminated(parameter, ascii_char(ENQ)),
        CommandToken::ReadParameter,
    )(buf)
}

fn bcc_fields<'a, E: ParseError<&'a Buf>>(s: &'a Buf) -> IResult<&'a Buf, (Parameter, Value), E> {
    terminated(tuple((parameter, x328_value)), ascii_char(ETX))(s)
}

fn received_bcc<'a, E: ParseError<&'a Buf>>(buf: &'a Buf) -> IResult<&'a Buf, u8, E> {
    map(take(1usize), |u: &Buf| u.as_bytes()[0])(buf)
}

fn param_data_etx(buf: &Buf) -> IResult<&Buf, (Parameter, Value, bool)> {
    let (buf, _stx) = ascii_char(SOX)(buf)?;

    let (buf, ((param, value), bcc_slice)) = pair(peek(bcc_fields), recognize(bcc_fields))(buf)?;
    let (buf, recv_bcc) = received_bcc(buf)?;
    let calc_bcc = bcc(bcc_slice);
    // println!("Write command checksum mismatch expected {}, got {}", calc_bcc, recv_bcc);
    Ok((buf, (param, value, calc_bcc == recv_bcc)))
}

fn write_command(buf: &Buf) -> IResult<&Buf, CommandToken> {
    match param_data_etx(buf) {
        Ok((buf, (param, value, true))) => Ok((buf, WriteParameter(param, value))),
        Ok((buf, (_param, _value, false))) => Ok((buf, SendNAK)),
        Err(Incomplete(x)) => Err(Incomplete(x)),
        Err(_) => Ok((buf, SendNAK)),
    }
}

fn ascii_char<'a, E: ParseError<&'a Buf>>(
    ascii_char: AsciiChar,
) -> impl Fn(&'a Buf) -> IResult<&'a Buf, char, E> {
    use nom::character::streaming;
    streaming::char(ascii_char.as_char())
}

fn map_int<'a, O, E, F>(first: F) -> impl Fn(&'a Buf) -> IResult<&'a Buf, O, E>
where
    E: ParseError<&'a Buf>,
    F: Fn(&'a Buf) -> IResult<&'a Buf, &'a Buf, E>,
    O: std::str::FromStr,
{
    //let to_str = map_res(first, |u: &'a Buf| std::str::from_utf8(u));  // for [u8] buffer
    let to_str = first;
    map_res(to_str, |s| s.parse::<O>())
}

fn bcc(s: &Buf) -> u8 {
    let mut ret = 0;
    for byte in s.bytes() {
        ret ^= byte;
    }
    if ret < 0x20 {
        ret += 0x20
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use nom::error::VerboseError;

    use ascii::AsciiChar::{Space, EOT, SOX};
    use ascii::{AsAsciiStr, AsciiString};
    use nom::Needed::Size;

    macro_rules! incomplete {
        ($x: expr) => {
            Err(Incomplete(Size($x)))
        };
    }

    #[test]
    fn test_address() {
        assert_eq!(address::<VerboseError<&Buf>>("11223"), Ok(("3", Valid(12))));
        assert_eq!(address::<VerboseError<&Buf>>("aa22"), Ok(("", Invalid)));
        assert_eq!(address::<VerboseError<&Buf>>("122"), incomplete!(4));
    }

    #[test]
    fn test_write() {
        let mut cmd = AsciiString::new();
        macro_rules! push {
            ($x:expr) => {
                cmd.push_str($x.as_ascii_str().unwrap());
            };
        }
        macro_rules! write {
            () => {
                write_command(cmd.as_str())
            };
        }

        cmd.push(SOX);

        assert_eq!(write!(), incomplete!(4));

        push!("1234123456\x03");
        assert_eq!(write!(), incomplete!(1)); // missing bcc

        push!(" ");
        assert_eq!(write!(), Ok(("", WriteParameter(1234, 123456))));
        let x = cmd.len() - 1;
        cmd[x] = EOT;
        assert_eq!(write!(), Ok(("", SendNAK)));

        cmd[x] = Space;
        push!("asd");
        assert_eq!(write!(), Ok(("asd", WriteParameter(1234, 123456))));
    }

    #[test]
    fn test_read_until_eot() {
        let mut cmd = AsciiString::new();
        macro_rules! push {
            ($x:expr) => {
                cmd.push_str($x.as_ascii_str().unwrap());
            };
        }
        macro_rules! rue {
            () => {
                read_until_eot(cmd.as_str())
            };
        }

        push!("asdjkhalksdjfhalskdfjha");
        assert_eq!(rue!(), incomplete!(1));
        cmd.push(EOT);
        assert_eq!(rue!(), incomplete!(4));
        push!("1122123");
        assert_eq!(rue!(), Ok(("123", Reset(Valid(12)))));
    }
}
