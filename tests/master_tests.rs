mod common;

use ascii::AsciiChar::{ACK, SOX};
use common::*;
use x328_proto::master::{io, WriteResponse};
use x328_proto::X328Error;
use x328_proto::{Address, Parameter};

#[test]
fn master_main_loop() {
    let data_in = [SOX.as_byte(), ACK.as_byte()];
    let serial_sim = SerialInterface::new(&data_in);
    let mut serial = SerialIOPlane::new(&serial_sim);
    // let mut registers: HashMap<Parameter, Value> = HashMap::new();

    let mut master = io::Master::new(&mut serial);
    let addr10 = Address::new_unchecked(10);
    let param20 = Parameter::new_unchecked(20);
    let x = master.write_parameter(addr10, param20, 3);
    assert_eq!(x.unwrap(), WriteResponse::TransmissionError);
    serial_sim.borrow_mut().trigger_write_error();
    let x = master.write_parameter(addr10, param20, 3);
    assert_eq!(x.unwrap_err(), X328Error::IOError);
    let x = master.write_parameter(addr10, param20, 3);
    println!("Write success: {:?}", x);
    assert_eq!(x.unwrap(), WriteResponse::WriteOk);
}
