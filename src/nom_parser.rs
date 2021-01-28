use nom::branch::alt;
use nom::bytes::streaming::{take_while, take_while_m_n};
use nom::combinator::{consumed, map, map_res, opt, value, verify};
use nom::number::streaming::u8;
use nom::sequence::{preceded, terminated, tuple};
use nom::Err::Incomplete;
use nom::IResult;

use crate::ascii::*;
use crate::types::{Address, Parameter, ParameterOffset, Value};

type Char = u8;
type Buf = [u8];

pub(crate) mod master {
    use super::*;
    use nom::combinator::all_consuming;

    #[derive(PartialEq, Copy, Clone, Debug)]
    pub(crate) enum ResponseToken {
        WriteOk,
        WriteFailed,
        InvalidParameter,
        ReadOK { parameter: Parameter, value: Value },
        NeedData,
        InvalidDataReceived,
    }

    pub(crate) fn parse_write_reponse(buf: &Buf) -> ResponseToken {
        parse_response(all_consuming(alt((
            value(ResponseToken::WriteOk, ascii_char(ACK)),
            value(ResponseToken::WriteFailed, ascii_char(NAK)),
            value(ResponseToken::InvalidParameter, ascii_char(EOT)),
        )))(buf))
    }

    pub(crate) fn parse_read_response(buf: &Buf) -> ResponseToken {
        parse_response(all_consuming(alt((
            value(ResponseToken::InvalidParameter, ascii_char(EOT)),
            map(stx_param_value_etx_bcc, |(parameter, value)| {
                ResponseToken::ReadOK { parameter, value }
            }),
        )))(buf))
    }

    fn parse_response(alt_match: IResult<&Buf, ResponseToken>) -> ResponseToken {
        match alt_match {
            Ok((_buf, token)) => token,
            Err(Incomplete(_)) => ResponseToken::NeedData,
            Err(_) => ResponseToken::InvalidDataReceived,
        }
    }
}

pub(crate) mod slave {
    use super::*;
    use CommandToken::*;

    #[derive(PartialEq, Debug, Copy, Clone)]
    pub(crate) enum CommandToken {
        WriteParameter(Address, Parameter, Value),
        ReadParameter(Address, Parameter),
        ReadAgain(ParameterOffset),
        InvalidPayload(Address),
        NeedData,
    }

    pub(crate) fn parse_command(buf: &Buf) -> (usize, CommandToken) {
        match alt_match(buf) {
            Ok((remaining, token)) => (buf.len() - remaining.len(), token),
            Err(Incomplete(_)) => (0, CommandToken::NeedData),
            Err(_) => panic!("Wut??"),
        }
    }

    fn alt_match(buf: &Buf) -> IResult<&Buf, CommandToken> {
        alt((
            read_again,
            write_command,
            read_command,
            invalid_payload,
            read_until_eot,
        ))(buf)
    }

    fn read_until_eot(buf: &Buf) -> IResult<&Buf, CommandToken> {
        let (buf, _) = take_while(|c| c != EOT)(buf)?;
        alt_match(buf)
    }

    fn read_command(buf: &Buf) -> IResult<&Buf, CommandToken> {
        let (buf, address) = eot_address(buf)?;
        let (buf, parameter) = terminated(parameter, ascii_char(ENQ))(buf)?;
        Ok((buf, ReadParameter(address, parameter)))
    }

    fn write_command(buf: &Buf) -> IResult<&Buf, CommandToken> {
        let (buf, address) = eot_address(buf)?;
        let (buf, (param, value)) = stx_param_value_etx_bcc(buf)?;
        Ok((buf, WriteParameter(address, param, value)))
    }

    fn read_again(buf: &Buf) -> IResult<&Buf, CommandToken> {
        alt((
            value(ReadAgain(1), ascii_char(ACK)),
            value(ReadAgain(0), ascii_char(NAK)),
            value(ReadAgain(-1), ascii_char(BS)),
        ))(buf)
    }

    fn invalid_payload(buf: &Buf) -> IResult<&Buf, CommandToken> {
        let (buf, _eot) = ascii_char(EOT)(buf)?;
        if let (_buf, Some(addr)) = opt(address)(buf)? {
            Ok((b"", InvalidPayload(addr)))
        } else {
            Ok((b"", NeedData))
        }
    }

    fn eot_address(buf: &Buf) -> IResult<&Buf, Address> {
        preceded(ascii_char(EOT), address)(buf)
    }

    fn address(buf: &Buf) -> IResult<&Buf, Address> {
        map_res(
            verify(
                take_while_m_n(4, 4, |c: Char| c.is_ascii_digit()),
                |x: &Buf| x[0] == x[1] && x[2] == x[3],
            ),
            |x: &Buf| Address::new((x[1] - b'0') * 10 + x[2] - b'0'),
        )(buf)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::ascii::EOT;
        use crate::buffer::Buffer;

        use nom::Needed;

        macro_rules! incomplete {
            ($x: expr) => {
                Err(Incomplete(Needed::new($x)))
            };
        }

        #[test]
        fn test_parse_command() {
            use slave::*;
            let mut buf = Buffer::new();
            buf.write(b"0");
            assert_eq!(parse_command(buf.as_ref()), (0, NeedData));

            assert_eq!(parse_command(b"\x15"), (1, ReadAgain(0)));
            assert_eq!(parse_command(b"\x08"), (1, ReadAgain(-1)));
            assert_eq!(parse_command(b"\x06"), (1, ReadAgain(1)));
        }

        #[test]
        fn test_address() {
            use slave::address;
            assert!(address(b"11223") == Ok((b"3", Address::new(12).unwrap())));
            assert!(address(b"1132").is_err());
            assert!(address(b"aa22").is_err());
            assert_eq!(address(b"122"), incomplete!(1));
        }

        #[test]
        fn test_write_command() {
            let mut cmd = Vec::<u8>::new();
            let addr = Address::new(10).unwrap();
            let param = Parameter::new(1234).unwrap();
            let value = Value::new(12345).unwrap();

            macro_rules! push {
                ($x:expr) => {
                    cmd.extend_from_slice($x);
                };
            }
            macro_rules! write {
                () => {
                    write_command(cmd.as_ref())
                };
            }

            cmd.push(EOT);
            push!(&addr.to_bytes());
            cmd.push(STX);

            assert_eq!(write!(), incomplete!(4));

            push!(b"123412345\x03");
            assert_eq!(write!(), incomplete!(1)); // missing bcc

            let correct_bcc = crate::bcc(&(cmd.as_slice()[6..]));
            cmd.push(correct_bcc);
            assert!(write!() == Ok((b"", WriteParameter(addr, param, value))));
            let x = cmd.len() - 1;
            cmd[x] = EOT; // Invalid BCC
            assert_eq!(
                parse_command(cmd.as_ref()),
                (cmd.len(), InvalidPayload(addr))
            );

            cmd[x] = correct_bcc; // Valid BCC
            push!(b"asd");
            assert!(write!() == Ok((b"asd", WriteParameter(addr, param, value))));
        }

        #[test]
        fn test_read_until_eot() {
            let mut cmd = Vec::<u8>::new();
            macro_rules! push {
                ($x:expr) => {
                    cmd.extend_from_slice($x.as_bytes());
                };
            }
            macro_rules! rue {
                () => {
                    read_until_eot(cmd.as_slice())
                };
            }

            push!("asdjkhalksdjfhalskdfjha");
            assert_eq!(rue!(), incomplete!(1));
            cmd.push(EOT);
            assert_eq!(rue!(), incomplete!(4));
            push!("1122123");
            assert_eq!(rue!(), incomplete!(1));
        }
    }
}

fn parameter(buf: &Buf) -> IResult<&Buf, Parameter> {
    map_int(take_while_m_n(4, 4, |c: Char| c.is_ascii_digit()))(buf)
}

fn x328_value(buf: &Buf) -> IResult<&Buf, Value> {
    terminated(
        map_int(take_while_m_n(1, 6, |c: Char| {
            c.is_ascii_digit() || c == b'+' || c == b'-'
        })),
        ascii_char(ETX),
    )(buf)
}

fn stx_param_value_etx_bcc(buf: &Buf) -> IResult<&Buf, (Parameter, Value)> {
    let (buf, _stx) = ascii_char(STX)(buf)?;
    let (buf, (bcc_slice, (param, value))) = consumed(tuple((parameter, x328_value)))(buf)?;
    let (buf, _) = verify(u8, |recv_bcc| bcc(bcc_slice) == *recv_bcc)(buf)?;
    Ok((buf, (param, value)))
}

fn ascii_char<'a>(ascii_char: u8) -> impl Fn(&'a Buf) -> IResult<&'a Buf, char> {
    nom::character::streaming::char(ascii_char as char)
}

fn map_int<'a, O, F>(first: F) -> impl FnMut(&'a Buf) -> IResult<&'a Buf, O>
where
    F: Fn(&'a Buf) -> IResult<&'a Buf, &'a Buf>,
    O: std::str::FromStr,
{
    // for [u8] buffer
    let to_str = map_res(first, |u: &'a Buf| std::str::from_utf8(u));
    // for &str buffer
    //let to_str = first;
    map_res(to_str, |s| s.parse::<O>())
}

fn bcc(s: &Buf) -> u8 {
    crate::bcc(s)
}

#[cfg(test)]
mod tests {}
