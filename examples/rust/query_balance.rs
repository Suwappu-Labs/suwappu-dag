//! Example: fetch a substrate balance for a 20-byte address.
//!
//! Run:
//!     cd examples/rust && cargo run --bin query_balance -- \
//!         0x0101010101010101010101010101010101010101
//!
//! Set `GSX_RPC_URL` to point at a non-local endpoint (e.g. the
//! public devnet): `GSX_RPC_URL=https://rpc.devnet.gsx.globalsettlement.com`.
//! Defaults to `http://127.0.0.1:9092`.
//!
//! With no address argument, queries the zero address (which always
//! returns "0" on a fresh devnet — useful as a smoke test).

use anyhow::{anyhow, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let addr_hex = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".into());

    let stripped = addr_hex
        .strip_prefix("0x")
        .or_else(|| addr_hex.strip_prefix("0X"))
        .unwrap_or(&addr_hex);
    let bytes = hex::decode(stripped).map_err(|e| anyhow!("bad hex address: {}", e))?;
    if bytes.len() != 20 {
        return Err(anyhow!(
            "address must be 20 bytes, got {} bytes",
            bytes.len()
        ));
    }
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes);

    let rpc_url = std::env::var("GSX_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:9092".into());
    let client = gsx_client::Client::new(&rpc_url);
    let view = client.get_balance(addr).await?;
    println!(
        "address : {}\nbalance : {} (decimal string; lift to u128 for math)",
        addr_hex, view.balance,
    );
    Ok(())
}
