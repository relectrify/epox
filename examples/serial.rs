use std::io::Read;

fn main() {
    let mut serial = epox::Fd::new(
        serialport::new("/dev/ttyUSB0", 115200)
            .open_native()
            .unwrap(),
        epox::Events::IN,
    )
    .unwrap();
    epox::spawn(async move {
        println!("waiting for serial data...");
        serial.ready().await;
        let mut buf = [0; 32];
        let len = serial.read(&mut buf).unwrap();
        println!("read {:#?}", &buf[0..len]);
    });
    epox::run().unwrap();
}
