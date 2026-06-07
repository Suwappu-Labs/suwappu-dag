//! Example: fetch the current epoch from a running devnet's JSON-RPC.
//!
//! Run:
//!     cd examples/rust && cargo run --bin query_epoch
//!
//! Set `SUWAPPU_RPC_URL` to point at a non-local endpoint (e.g. the
//! public devnet): `SUWAPPU_RPC_URL=https://rpc.devnet.suwappu.globalsettlement.com`.
//! Defaults to `http://127.0.0.1:9092`. See ../../DEVNET.md for the
//! local devnet flow.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("SUWAPPU_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:9092".into());
    let client = suwappu_client::Client::new(&rpc_url);
    let epoch = client.get_epoch().await?;
    println!(
        "current epoch         : {}\n\
         last boundary round   : {}\n\
         rounds per epoch      : {}",
        epoch.current, epoch.last_boundary_round, epoch.rounds_per_epoch,
    );
    Ok(())
}
