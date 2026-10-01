use clap::Parser;
use rm_server_sync::{Service, http};
use std::net::SocketAddr;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    address: SocketAddr,
    // 上界一年：再大的取值会让 `Instant::now() + ttl` 在登录时溢出 panic，而那个
    // panic 发生在持有 users 锁的临界区里，会把互斥锁毒化。
    #[arg(long, default_value = "300", value_parser = clap::value_parser!(u64).range(1..=365 * 24 * 60 * 60))]
    token_ttl_seconds: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    http::run(args.address, Service::new(args.token_ttl_seconds))
}
