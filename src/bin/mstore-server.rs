use std::sync::Arc;

use clap::Parser;
use mstore::{default_endpoint, MStoreServer};

#[derive(Debug, Parser)]
#[command(
    name = "mstore-server",
    version,
    about = "Mutable shared-memory object server"
)]
struct Args {
    /// unix:///path/to.sock. TCP is intentionally rejected on Unix because it cannot carry FDs.
    #[arg(long, default_value_t = default_endpoint())]
    endpoint: String,

    /// Include extra internal detail in internal-error responses.
    #[arg(long)]
    debug: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let server = Arc::new(MStoreServer::new(Some(args.endpoint.clone()), args.debug)?);
    println!("mstore server starting on {}", server.endpoint());
    server.serve_forever()?;
    Ok(())
}
