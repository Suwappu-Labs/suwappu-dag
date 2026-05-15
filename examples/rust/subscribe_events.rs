//! Example: tail the devnet's commit stream via WebSocket.
//!
//! Run:
//!     cd examples/rust && cargo run --bin subscribe_events
//!
//! Prints one line per EventView until Ctrl-C. Pre-req: devnet up at
//! `ws://127.0.0.1:9092/ws`. See ../../DEVNET.md.

use anyhow::Result;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<()> {
    let url = "ws://127.0.0.1:9092/ws";
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
