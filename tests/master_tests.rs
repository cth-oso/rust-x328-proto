mod common;

use x328_proto::master::io;
use x328_proto::{Address, Parameter};

use common::bytes::*;
use common::*;

#[test]
fn master_main_loop() {
    let data_in = [STX, ACK];
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

#[test]
fn test_write() {
    let bus = RS422Bus::new();
    let mut master = io::Master::new(bus.new_master_interface());
    let mut response = bus.new_node_interface();
    response.putc(ACK);
    assert!(master.write_parameter(10, 20, 30).is_ok());
    assert!(master.write_parameter(100, 22, 32).is_err());
    assert!(master.write_parameter(20, 10000, 32).is_err());
    assert!(master.write_parameter(20, 1000, 70000).is_err());
    response.putc(ACK);
    assert!(master.write_parameter(42, 22, 32).is_ok());
}

#[test]
fn test_read() {
    let bus = RS422Bus::new();
    let mut master = io::Master::new(bus.new_master_interface());
    assert!(master.read_parameter(10, 20000).is_err());
    assert!(master.read_parameter(100, 2000).is_err());
}
