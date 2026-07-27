//! CLI: exercise the reserve-coverage circuit against a hardcoded demo
//! composition. `cargo run --release -- execute` (fast, no proof) or
//! `cargo run --release -- prove` (real proof, RAM-heavy — see lib.rs).

use reserve_coverage_host::{compute_reference, execute, prove, verify};

#[tokio::main]
async fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "execute".to_string());

    let salt = [0x5Au8; 32];
    let amounts: Vec<u128> = vec![10_000_000, 25_000_000, 7_500_000];
    let (expected_total, expected_commitment) = compute_reference(salt, &amounts);
    eprintln!(
        "reference: total_reserves={expected_total} commitment={}",
        hex::encode(expected_commitment)
    );

    match mode.as_str() {
        "execute" => {
            let (total_reserves, commitment) =
                execute(salt, &amounts).await.expect("execute failed");
            eprintln!(
                "execute:   total_reserves={total_reserves} commitment={}",
                hex::encode(commitment)
            );
            assert_eq!(total_reserves, expected_total);
            assert_eq!(commitment, expected_commitment);
            eprintln!("Real SP1 execution matches the reference computation. OK");
        }
        "prove" => {
            let (proof, vk) = prove(salt, &amounts).await.expect("prove failed");
            eprintln!("Proof generated. Verifying...");
            verify(&proof, &vk, expected_total, expected_commitment)
                .await
                .expect("verify failed");
            eprintln!("Real STARK proof generated and verified. OK");
        }
        other => eprintln!("unknown mode {other:?}; use 'execute' or 'prove'"),
    }
}
