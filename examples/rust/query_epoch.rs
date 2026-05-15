//! Example: fetch the current epoch from a running devnet's JSON-RPC.
//!
//! Run:
//!     cd examples/rust && cargo run --bin query_epoch
//!
//! Pre-req: a devnet up at `http://127.0.0.1:9092`. See ../../DEVNET.md.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let client = gsx_client::Client::new("http://127.0.0.1:9092");
    let epoch = client.get_epoch().await?;
    println!(
        "current epoch         : {}\n\
         last boundary round   : {}\n\
         rounds per epoch      : {}",
        epoch.current, epoch.last_boundary_round, epoch.rounds_per_epoch,
    );
    Ok(())
}
