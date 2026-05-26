//! Example: tail the devnet's commit stream via WebSocket.
//!
//! Run:
//!     cd examples/rust && cargo run --bin subscribe_events
//!
//! Set `GSX_RPC_URL` to point at a non-local endpoint (e.g. the
//! public devnet): `GSX_RPC_URL=https://rpc.devnet.gsx.globalsettlement.com`.
//! Defaults to `http://127.0.0.1:9092`. The example derives the WS
//! URL by swapping `http`→`ws` / `https`→`wss` and appending `/ws`.
//!
//! Prints one line per EventView until Ctrl-C. See ../../DEVNET.md.

use anyhow::Result;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("GSX_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:9092".into());
    let url = if let Some(rest) = rpc_url.strip_prefix("https://") {
        format!("wss://{}/ws", rest.trim_end_matches('/'))
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        format!("ws://{}/ws", rest.trim_end_matches('/'))
    } else {
        return Err(anyhow::anyhow!(
            "GSX_RPC_URL must start with http:// or https://, got: {}",
            rpc_url
        ));
    };
    println!("subscribing to {url} ...");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    println!("connected; tailing events (Ctrl-C to exit)\n");

    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(line) => {
                // The server emits one EventView JSON per frame, plus
                // an occasional `{"error":"lagged", ...}` notice when
                // its broadcast buffer overflows. Print the raw line —
                // a real consumer would parse via serde.
                println!("{line}");
            }
            Message::Close(reason) => {
                println!("\nws closed: {:?}", reason);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
