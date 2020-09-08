mod common;

use ascii::AsciiChar::{ACK, SOX};
use common::*;
use x328_proto::master::io;
use x328_proto::X328Error;
use x328_proto::{Address, Parameter};

#[test]
fn master_main_loop() {
    let data_in = [SOX.as_byte(), ACK.as_byte()];
    let serial_sim = SerialInterface::new(&data_in);
    let mut serial = SerialIOPlane::new(&serial_sim);
    // let mut registers: HashMap<Parameter, Value> = HashMap::new();

    let mut master = io::Master::new(&mut serial);
    let addr10 = Address::new(10).unwrap();
    let param20 = Parameter::new(20).unwrap();
    let x = master
        .write_parameter(addr10, param20, 3)
        .expect_err("Should be transmission error, SOX received");
    assert_eq!(x, X328Error::IOError);
    serial_sim.borrow_mut().trigger_write_error();
    let x = master.write_parameter(addr10, param20, 3);
    assert_eq!(x.unwrap_err(), X328Error::IOError);
    let x = master.write_parameter(addr10, param20, 3);
    println!("Write success: {:?}", x);
    assert_eq!(x, Ok(()));
}
