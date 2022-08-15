use anyhow::{Context, Result};
use serialport::{DataBits, Parity};
use std::io::{Read, Write};
use std::iter::Peekable;
use std::str::{FromStr, SplitWhitespace};
use std::sync::mpsc;

use x328_proto::master::io::Master;

fn cmd_read<IO: Read + Write>(args: &mut CmdScanner, x328: &mut Master<IO>) -> Result<()> {
    println!(
        "{}",
        *x328.read_parameter(args.parse_next::<u8>()?, args.parse_next::<u16>()?)?
    );
    Ok(())
}

fn cmd_poll<IO: Read + Write>(args: &mut CmdScanner, x328: &mut Master<IO>) -> Result<()> {
    let addr: u8 = args.parse_next()?;
    let param: u16 = args.parse_next()?;
    let delay = std::time::Duration::from_secs_f32(args.parse_next()?);

    println!("Press enter to stop polling.");
    // check that the first read is ok before starting the poll stop thread
    println!("{}", *x328.read_parameter(addr, param)?);
    let (io_tx, io_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ch = io_tx;
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
    });
    loop {
        if io_rx.recv_timeout(delay) == Err(mpsc::RecvTimeoutError::Disconnected) {
            break;
        }
        println!("{}", *x328.read_parameter(addr, param)?);
    }
    Ok(())
}

fn cmd_write<IO: Read + Write>(args: &mut CmdScanner, x328: &mut Master<IO>) -> Result<()> {
    x328.write_parameter(
        args.parse_next::<u8>()?,
        args.parse_next::<u16>()?,
        args.parse_next::<i32>()?,
    )?;
    Ok(())
}

fn main() -> () {
    env_logger::init();

    let mut args = std::env::args();
    args.next(); // Skip program name
    let port = args.next().unwrap_or("/dev/ttyACM0".to_string());

    let serial = serialport::new(&port, 9600)
        .data_bits(DataBits::Seven)
        .parity(Parity::Even)
        .timeout(std::time::Duration::from_millis(100))
        .open()
        .expect("Failed to open serial port");

    let mut stdout = std::io::stdout();

    let mut x328 = Master::new(serial);
    loop {
        print!(">> ");
        stdout.flush().unwrap();
        let mut cmd = String::new();
        let mut scan = CmdScanner::read_stdin(&mut cmd);
        if let Err(err) = match scan.next() {
            Err(_) => continue,
            Ok("read") | Ok("r") => cmd_read(&mut scan, &mut x328),
            Ok("poll") => cmd_poll(&mut scan, &mut x328),
            Ok("write") => cmd_write(&mut scan, &mut x328),
            Ok(cmd) => {
                println!("Unknown command {}", cmd);
                continue;
            }
        } {
            println!("{:?}", err)
        }
    }
}

struct CmdScanner<'a> {
    splt: Peekable<SplitWhitespace<'a>>,
}

impl<'a> CmdScanner<'a> {
    fn read_stdin(buf: &'a mut String) -> Self {
        buf.clear();
        std::io::stdin().read_line(buf).unwrap();
        let splt = buf.split_whitespace().peekable();
        Self { splt }
    }
    fn next(&mut self) -> Result<&str> {
        self.splt.next().context("End of stream")
    }
    fn parse_next<T: FromStr>(&mut self) -> Result<T> {
        self.next()?.parse::<T>().ok().context("Parse error")
    }
}
