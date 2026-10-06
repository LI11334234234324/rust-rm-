use clap::Parser;
use rm_server_async::{Service, http::with_service};
use std::net::SocketAddr;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    address: SocketAddr,
    #[arg(long, default_value = "300", value_parser = clap::value_parser!(u64).range(1..))]
    token_ttl_seconds: u64,
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let app = with_service(Service::new(args.token_ttl_seconds));
    let config = app
        .figment()
        .clone()
        .merge(("address", args.address.ip()))
        .merge(("port", args.address.port()))
        .merge(("log_level", "critical"));
    app.configure(config).launch().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::Parser;

    #[test]
    fn token_ttl_defaults_to_300_seconds() {
        let args = Args::try_parse_from(["server"]).unwrap();
        assert_eq!(args.token_ttl_seconds, 300);
    }

    #[test]
    fn token_ttl_accepts_positive_u64_values() {
        for ttl in [1, 31_536_000, 31_536_001, u64::MAX] {
            let value = ttl.to_string();
            let args = Args::try_parse_from(["server", "--token-ttl-seconds", &value]).unwrap();
            assert_eq!(args.token_ttl_seconds, ttl);
        }
    }

    #[test]
    fn token_ttl_rejects_invalid_values() {
        for value in ["0", "-1", "1.5", "invalid", "18446744073709551616"] {
            assert!(Args::try_parse_from(["server", "--token-ttl-seconds", value]).is_err());
        }
    }
}
