use nom::branch::alt;
use nom::bytes::streaming::take_while_m_n;
use nom::combinator::{consumed, map, map_res, opt, value, verify};
use nom::number::streaming::u8;
use nom::sequence::{preceded, terminated, tuple};
use nom::Err::Incomplete;
use nom::IResult;

use crate::ascii::*;
use crate::types::{Address, Parameter, ParameterOffset, Value};
use std::convert::TryInto;

type Char = u8;
type Buf = [u8];

pub mod master {
    use super::*;
    use nom::combinator::all_consuming;

    #[derive(PartialEq, Copy, Clone, Debug)]
    pub enum ResponseToken {
        WriteOk,
        WriteFailed,
        InvalidParameter,
        ReadOk { parameter: Parameter, value: Value },
        NeedData,
        InvalidDataReceived,
    }

    pub fn parse_write_response(buf: &Buf) -> ResponseToken {
        parse_response(all_consuming(alt((
            value(ResponseToken::WriteOk, ascii_char(ACK)),
            value(ResponseToken::WriteFailed, ascii_char(NAK)),
            value(ResponseToken::InvalidParameter, ascii_char(EOT)),
        )))(buf))
    }

    pub fn parse_read_response(buf: &Buf) -> ResponseToken {
        parse_response(all_consuming(alt((
            value(ResponseToken::InvalidParameter, ascii_char(EOT)),
            map(stx_param_value_etx_bcc, |(parameter, value)| {
                ResponseToken::ReadOk { parameter, value }
            }),
        )))(buf))
    }

    const fn parse_response(alt_match: IResult<&Buf, ResponseToken>) -> ResponseToken {
        match alt_match {
            Ok((_buf, token)) => token,
            Err(Incomplete(_)) => ResponseToken::NeedData,
            Err(_) => ResponseToken::InvalidDataReceived,
        }
    }
}

pub mod node {
    use super::*;
    use CommandToken::*;

    #[derive(PartialEq, Debug, Copy, Clone)]
    pub enum CommandToken {
        WriteParameter(Address, Parameter, Value),
        ReadParameter(Address, Parameter),
        ReadAgain(ParameterOffset),
        InvalidPayload(Address),
        NeedData,
    }

    pub fn parse_command(buf: &Buf) -> (usize, CommandToken) {
        let (remaining, token) = alt_match(buf);
        (buf.len() - remaining.len(), token)
    }

    fn alt_match(buf: &Buf) -> (&Buf, CommandToken) {
        if let Ok(x) = read_again(buf) {
            return x;
        }
        let buf = find_last_eot(buf);
        alt((write_command, read_command, invalid_payload))(buf)
            .unwrap_or((buf, CommandToken::NeedData))
    }

    /// Consumes the buffer until the last EOT is found
    fn find_last_eot(buf: &Buf) -> &Buf {
        buf.iter()
            .rposition(|c| *c == EOT)
            .map_or(b"", |pos| &buf[pos..])
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
        let (buf, addr) = preceded(ascii_char(EOT), opt(address))(buf)?;
        let buf = find_last_eot(buf);
        let tok = addr.map_or(CommandToken::NeedData, CommandToken::InvalidPayload);
        Ok((buf, tok))
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
            use node::*;
            let mut buf = Buffer::new();
            buf.write(b"0");
            assert_eq!(parse_command(buf.as_ref()), (1, NeedData));

            assert_eq!(parse_command(b"\x15"), (1, ReadAgain(0)));
            assert_eq!(parse_command(b"\x08"), (1, ReadAgain(-1)));
            assert_eq!(parse_command(b"\x06"), (1, ReadAgain(1)));
        }

        #[test]
        fn test_address() {
            use node::address;
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
            let value: Value = 12345_u16.into();

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
            cmd[x] = correct_bcc + 1; // Invalid BCC
            assert_eq!(
                parse_command(cmd.as_ref()),
                (cmd.len(), InvalidPayload(addr))
            );

            cmd[x] = correct_bcc; // Valid BCC
            push!(b"asd");
            assert!(write!() == Ok((b"asd", WriteParameter(addr, param, value))));
        }
    }
}

fn parameter(buf: &Buf) -> IResult<&Buf, Parameter> {
    map_res(
        take_while_m_n(4, 4, |c: Char| c.is_ascii_digit()),
        |b: &Buf| b.try_into(),
    )(buf)
}

fn x328_value(buf: &Buf) -> IResult<&Buf, Value> {
    terminated(
        map_res(
            take_while_m_n(1, 6, |c: Char| c.is_ascii_digit() || c == b'+' || c == b'-'),
            |b: &Buf| b.try_into(),
        ),
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

fn bcc(s: &Buf) -> u8 {
    crate::bcc(s)
}

#[cfg(test)]
mod test_public_interface {
    use crate::ascii::*;
    use crate::bcc;

    /// Push parameter, value, bcc to the buffer
    macro_rules! push_spveb {
        ($buf:expr, $param:expr, $value:expr) => {
            $buf.push(STX);
            let bcc_start = $buf.len();
            $buf.extend_from_slice($param);
            $buf.extend_from_slice($value);
            $buf.push(ETX);
            $buf.push(bcc(&($buf)[bcc_start..]));
        };
    }

    #[test]
    fn read_command() {
        use super::node::{parse_command, CommandToken};

        let mut buf = vec![EOT];
        buf.extend_from_slice(b"1199"); // address
        buf.extend_from_slice(b"0010"); // parameter
        let enq_pos = buf.len();
        buf.push(ENQ);

        // Valid read command, with trailing data
        match parse_command(&buf) {
            (10, CommandToken::ReadParameter(addr, param)) => {
                assert_eq!(addr, 19);
                assert_eq!(param, 10);
            }
            tok => panic!("Invalid token {:?}", tok),
        }

        // Valid command, short read
        for len in 0..enq_pos {
            assert_eq!(parse_command(&buf[..len]), (0, CommandToken::NeedData));
        }

        // Corrupted parameter or ENQ byte
        for n in 5..=enq_pos {
            let old = buf[n];
            buf[n] = b'A';
            match parse_command(&buf) {
                (consumed, CommandToken::InvalidPayload(addr)) => {
                    assert_eq!(addr, 19);
                    assert_eq!(consumed, enq_pos + 1);
                }
                tok => panic!("Invalid token {:?}", tok),
            }
            buf[n] = old;
        }

        // corrupted EOT
        buf[0] += 1;
        match parse_command(&buf) {
            (10, CommandToken::NeedData) => {}
            tok => panic!("Invalid token {:?}", tok),
        }
        buf[0] -= 1;
        // corrupted address
        buf[1] += 1;
        match parse_command(&buf) {
            (10, CommandToken::NeedData) => {}
            tok => panic!("Invalid token {:?}", tok),
        }
        buf[1] -= 1;
    }

    #[test]
    /// Test that parsing recovers if a command is interrupted
    /// and a new command is transmitted
    fn overlapping_commands() {
        use super::node::{parse_command, CommandToken};

        let mut read_cmd = vec![EOT];
        read_cmd.extend_from_slice(b"1199"); // address
        read_cmd.extend_from_slice(b"0010"); // parameter
        read_cmd.push(ENQ);

        for brk in 1..(read_cmd.len() - 1) {
            let buf: Vec<_> = read_cmd[..brk]
                .iter()
                .copied()
                .chain(read_cmd.iter().copied())
                .collect();
            match parse_command(&buf) {
                (consumed, CommandToken::ReadParameter(_, _)) => assert_eq!(consumed, buf.len()),
                t => panic!("{:?}", t),
            }
        }
    }

    #[test]
    fn read_response() {
        use super::master::{parse_read_response, ResponseToken};

        let mut buf = Vec::new();
        push_spveb!(buf, b"1234", b"-54321");

        let bcc_pos = buf.len() - 1;
        macro_rules! invalid_data {
            ($pre:expr, $post:expr) => {
                $pre;
                assert_eq!(
                    parse_read_response(&buf),
                    ResponseToken::InvalidDataReceived
                );
                $post;
            };
        }

        // Valid response
        match parse_read_response(&buf) {
            ResponseToken::ReadOk { parameter, value } => {
                assert_eq!(parameter, 1234);
                assert_eq!(value, -54321);
            }
            _ => panic!("Invalid response"),
        }

        // Valid response, short read
        for len in 0..(buf.len() - 1) {
            let x = parse_read_response(&buf[..len]);
            assert_eq!(x, ResponseToken::NeedData);
        }

        // Trailing data
        invalid_data!(buf.push(0), buf.pop());

        // BCC checksum mismatch
        invalid_data!(buf[bcc_pos] += 1, buf[bcc_pos] -= 1);

        // STX -> NAK
        invalid_data!(buf[0] = NAK, buf[0] = STX);

        // STX -> EOT
        invalid_data!(buf[0] = EOT, buf[0] = STX);

        // bad parameter
        assert_eq!(parse_read_response(&[EOT]), ResponseToken::InvalidParameter);
        assert_eq!(
            parse_read_response(&[EOT, EOT]),
            ResponseToken::InvalidDataReceived
        );
    }

    #[test]
    fn write_command() {
        use super::node::{parse_command, CommandToken};

        let mut buf = vec![EOT];
        buf.extend_from_slice(b"1199"); // address
        let stx_pos = buf.len();
        push_spveb!(buf, b"1234", b"-54321");
        let cmd_len = buf.len();

        // Valid command
        match parse_command(&buf) {
            (consumed, CommandToken::WriteParameter(addr, param, val)) => {
                assert_eq!(consumed, cmd_len);
                assert_eq!(addr, 19);
                assert_eq!(param, 1234);
                assert_eq!(val, -54321);
            }
            x => panic!("{:?}", x),
        };

        // Valid command, short read
        for n in 0..(cmd_len - 1) {
            assert_eq!(parse_command(&buf[..n]), (0, CommandToken::NeedData));
        }

        // Corrupt EOT or addr
        for n in 0..stx_pos {
            buf[n] += 1;
            assert_eq!(parse_command(&buf), (cmd_len, CommandToken::NeedData));
            buf[n] -= 1;
        }

        // Corrupt payload
        for n in stx_pos..cmd_len {
            buf[n] += 3; // +1 turns ETX => EOT, which gives NeedData instead of InvalidPayload
            match parse_command(&buf) {
                (consumed, CommandToken::InvalidPayload(addr))
                    if consumed == cmd_len && addr == 19 => {}
                x => panic!("{:?} => {:?}", String::from_utf8_lossy(&buf), x),
            }
            buf[n] -= 3;
        }
    }

    #[test]
    fn write_response() {
        use super::master::{parse_write_response, ResponseToken};

        for b in 0u8..=255 {
            match parse_write_response(&[b]) {
                ResponseToken::WriteOk if b == ACK => {}
                ResponseToken::WriteFailed if b == NAK => {}
                ResponseToken::InvalidParameter if b == EOT => {}
                ResponseToken::InvalidDataReceived if ![ACK, NAK, EOT].contains(&b) => {}
                tok => panic!("Invalid response token {} => {:?}", b, tok),
            }
        }

        assert_eq!(
            parse_write_response(&[ACK, ACK]),
            ResponseToken::InvalidDataReceived
        );
    }
}
