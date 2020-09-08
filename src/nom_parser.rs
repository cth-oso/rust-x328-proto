use ascii::AsciiChar::{self, BackSpace, ACK, ENQ, EOT, ETX, NAK, SOX as STX};

use nom::error::ParseError;
use nom::Err::Incomplete;

use nom::branch::alt;
use nom::bytes::streaming::{take, take_while, take_while_m_n};
use nom::sequence::{pair, preceded, terminated, tuple};
use nom::IResult;

use nom::combinator::{map, map_res, opt, peek, recognize, value, verify};

use crate::types::{Address, Parameter, ParameterOffset, Value};
use crate::X328Error;

type Buf = str;

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
        let (buf, _) = take_while(|c| c != EOT.as_char())(buf)?;
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
            value(ReadAgain(-1), ascii_char(BackSpace)),
        ))(buf)
    }

    fn invalid_payload(buf: &Buf) -> IResult<&Buf, CommandToken> {
        let (buf, _eot) = ascii_char(EOT)(buf)?;
        if let (_buf, Some(addr)) = opt(address)(buf)? {
            Ok(("", InvalidPayload(addr)))
        } else {
            Ok(("", NeedData))
        }
    }

    fn eot_address(buf: &Buf) -> IResult<&Buf, Address> {
        preceded(ascii_char(EOT), address)(buf)
    }

    fn address(buf: &Buf) -> IResult<&Buf, Address> {
        map_res(
            take_while_m_n(4, 4, |c: char| c.is_ascii_digit()),
            |x: &str| {
                if x[0..1] == x[1..2] && x[2..3] == x[3..] {
                    Ok(x[1..3].parse::<Address>()?)
                } else {
                    Err(X328Error::InvalidAddress)
                }
            },
        )(buf)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::buffer::Buffer;
        use ascii::AsciiChar::{Space, EOT};
        use ascii::{AsAsciiStr, AsciiString};
        use nom::Needed::Size;

        macro_rules! incomplete {
            ($x: expr) => {
                Err(Incomplete(Size($x)))
            };
        }

        #[test]
        fn test_parse_command() {
            use slave::*;
            let mut buf = Buffer::new();
            buf.write(b"0");
            assert_eq!(parse_command(buf.as_str_slice()), (0, NeedData));

            assert_eq!(parse_command("\x15"), (1, ReadAgain(0)));
            assert_eq!(parse_command("\x08"), (1, ReadAgain(-1)));
            assert_eq!(parse_command("\x06"), (1, ReadAgain(1)));
        }

        #[test]
        fn test_address() {
            use slave::address;
            assert_eq!(address("11223"), Ok(("3", Address::new(12).unwrap())));
            assert!(address("1132").is_err());
            assert!(address("aa22").is_err());
            assert_eq!(address("122"), incomplete!(1));
        }

        #[test]
        fn test_write_command() {
            let mut cmd = AsciiString::new();
            let addr = Address::new(10).unwrap();
            let param = Parameter::new(1234).unwrap();

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

            cmd.push(EOT);
            push!(addr.to_bytes());
            cmd.push(STX);

            assert_eq!(write!(), incomplete!(4));

            push!("1234123456\x03");
            assert_eq!(write!(), incomplete!(1)); // missing bcc

            push!(" ");
            assert_eq!(write!(), Ok(("", WriteParameter(addr, param, 123456))));
            let x = cmd.len() - 1;
            cmd[x] = EOT; // Invalid BCC
            assert_eq!(
                parse_command(cmd.as_str()),
                (cmd.len(), InvalidPayload(addr))
            );

            cmd[x] = Space; // Valid BCC
            push!("asd");
            assert_eq!(write!(), Ok(("asd", WriteParameter(addr, param, 123456))));
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
            assert_eq!(rue!(), incomplete!(1));
        }
    }
}

fn parameter(buf: &Buf) -> IResult<&Buf, Parameter> {
    map_int(take_while_m_n(4, 4, |c: char| c.is_ascii_digit()))(buf)
}

fn x328_value(buf: &Buf) -> IResult<&Buf, Value> {
    map_int(take_while_m_n(1, 6, |c: char| {
        c.is_ascii_digit() || c == '+' || c == '-'
    }))(buf)
}

fn bcc_fields(s: &Buf) -> IResult<&Buf, (Parameter, Value)> {
    terminated(tuple((parameter, x328_value)), ascii_char(ETX))(s)
}

fn received_bcc(buf: &Buf) -> IResult<&Buf, u8> {
    map(take(1usize), |u: &Buf| u.as_bytes()[0])(buf)
}

fn stx_param_value_etx_bcc(buf: &Buf) -> IResult<&Buf, (Parameter, Value)> {
    let (buf, _stx) = ascii_char(STX)(buf)?;
    let (buf, ((param, value), bcc_slice)) = pair(peek(bcc_fields), recognize(bcc_fields))(buf)?;
    let (buf, _) = verify(received_bcc, |recv_bcc| bcc(bcc_slice) == *recv_bcc)(buf)?;
    Ok((buf, (param, value)))
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
    crate::bcc(s.as_bytes())
}

#[cfg(test)]
mod tests {}
