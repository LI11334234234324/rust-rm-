use clap::Parser;
use rm_server_async::{Service, http::with_service};
use std::net::SocketAddr;
use std::num::NonZeroU64;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    address: SocketAddr,
    #[arg(long, default_value = "300")]
    token_ttl_seconds: NonZeroU64,
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let app = with_service(Service::new(args.token_ttl_seconds.get()));
    let config = app
        .figment()
        .clone()
        .merge(("address", args.address.ip()))
        .merge(("port", args.address.port()))
        .merge(("log_level", "critical"));
    app.configure(config).launch().await?;
    Ok(())
}
