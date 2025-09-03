use clap::Parser;
use std::io::Read;

#[derive(Parser)]
struct Args {
    #[clap(default_value = "/dev/ttyUSB0")]
    port: String,
    #[clap(default_value_t = 115_200)]
    baud_rate: u32,
}

fn main() {
    let args = Args::parse();
    let mut serial = epox::Fd::new(
        serialport::new(args.port, args.baud_rate)
            .open_native()
            .unwrap(),
        epox::EpollFlags::EPOLLIN,
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
