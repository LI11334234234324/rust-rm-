use clap::Parser;
use rm_server_sync::{Service, http};
use std::net::SocketAddr;
use std::num::NonZeroU64;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    address: SocketAddr,
    #[arg(long, default_value = "300")]
    token_ttl_seconds: NonZeroU64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    http::run(args.address, Service::new(args.token_ttl_seconds.get()))
}
