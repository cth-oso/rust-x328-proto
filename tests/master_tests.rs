mod common;

use ascii::AsciiChar::{ACK, SOX};
use common::*;
use x328_proto::master::io;
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
    master
        .write_parameter(addr10, param20, 3)
        .expect_err("Should be transmission error, SOX received");
    serial_sim.borrow_mut().trigger_write_error();
    master
        .write_parameter(addr10, param20, 3)
        .expect_err("Bus write error should have resulted in Error response");
    let x = master.write_parameter(addr10, param20, 3);
    println!("Write success: {:?}", x);
    x.unwrap();
}
