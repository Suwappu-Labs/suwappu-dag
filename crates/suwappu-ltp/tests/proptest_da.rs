//! DAG-S16 exit-gate property tests.
//!
//! Exit gate: `da_sla_enforced` — for any (payload, SLA, retrieval
//! response) triple, `verify_sla` accepts iff the response satisfies
//! every term of the SLA: matching CID, latency within window, and
//! response within the retention window.
//!
//! Supporting properties:
//!
//! - `content_address_binding` — `Cid::of(payload)` is a pure function
//!   of bytes; identical bytes → identical CID, distinct bytes →
//!   distinct CIDs (collision-resistance of SHA3-256 assumed).
//! - `late_response_signals_breach` — `responded_at - requested_at >
//!   max_latency` returns `LatencyExceeded`.
//! - `forged_payload_signals_breach` — a payload whose CID does not
//!   match the commitment returns `PayloadMismatch`.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-ltp --release`.

use suwappu_ltp::{verify_sla, Cid, CommitmentNetwork, DaError, DaSla};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — `verify_sla` accepts iff every SLA term holds, and
    /// surfaces a precise error for the term that breaks.
    #[test]
    fn da_sla_enforced(
        payload in prop::collection::vec(any::<u8>(), 0..=4096),
        retention in 1u64..=10_000,
        max_latency in 1u32..=1_000,
        stored_at in 0u64..=10_000,
        request_offset in 0u64..=20_000,
        latency_offset in 0u64..=2_000,
    ) {
        let mut net = CommitmentNetwork::new();
        let sla = DaSla {
            retention_rounds: retention,
            max_retrieval_latency_rounds: max_latency,
        };
        let cid = net.store(payload.clone(), sla, stored_at);
        let (commitment, returned) = net.retrieve(cid).unwrap();

        let requested_at = stored_at + request_offset;
        let responded_at = requested_at + latency_offset;
        let actual = verify_sla(&commitment, requested_at, responded_at, &returned);

        let retention_until = stored_at + retention;
        if responded_at > retention_until {
            let is_retention_error = matches!(actual, Err(DaError::RetentionExpired { .. }));
            prop_assert!(is_retention_error);
        } else if latency_offset > max_latency as u64 {
            let is_latency_error = matches!(actual, Err(DaError::LatencyExceeded { .. }));
            prop_assert!(is_latency_error);
        } else {
            prop_assert!(actual.is_ok());
        }
    }

    /// Content addressing is a pure function of bytes — identical
    /// payloads collapse to identical CIDs; distinct payloads of
    /// non-trivial size are virtually certain to produce distinct
    /// CIDs (collision-resistance of SHA3-256).
    #[test]
    fn content_address_binding(
        payload_a in prop::collection::vec(any::<u8>(), 1..=2048),
        payload_b in prop::collection::vec(any::<u8>(), 1..=2048),
    ) {
        prop_assert_eq!(Cid::of(&payload_a), Cid::of(&payload_a));
        if payload_a != payload_b {
            prop_assert_ne!(Cid::of(&payload_a), Cid::of(&payload_b));
        }
    }

    /// A late retrieval response (latency strictly above the SLA
    /// window) is flagged as `LatencyExceeded` regardless of payload.
    #[test]
    fn late_response_signals_breach(
        payload in prop::collection::vec(any::<u8>(), 0..=512),
        max_latency in 1u32..=100,
        breach_excess in 1u64..=1_000,
        stored_at in 0u64..=100,
        requested_at_offset in 0u64..=100,
    ) {
        let mut net = CommitmentNetwork::new();
        let sla = DaSla {
            retention_rounds: 1_000_000, // huge; isolate the latency check
            max_retrieval_latency_rounds: max_latency,
        };
        let cid = net.store(payload.clone(), sla, stored_at);
        let (commitment, returned) = net.retrieve(cid).unwrap();
        let requested_at = stored_at + requested_at_offset;
        let responded_at = requested_at + max_latency as u64 + breach_excess;
        let err = verify_sla(&commitment, requested_at, responded_at, &returned);
        let is_latency_error = matches!(err, Err(DaError::LatencyExceeded { .. }));
        prop_assert!(is_latency_error);
    }

    /// A returned payload whose CID does not match the commitment
    /// is flagged as `PayloadMismatch`.
    #[test]
    fn forged_payload_signals_breach(
        original in prop::collection::vec(any::<u8>(), 1..=512),
        forged in prop::collection::vec(any::<u8>(), 1..=512),
    ) {
        prop_assume!(Cid::of(&original) != Cid::of(&forged));
        let mut net = CommitmentNetwork::new();
        let cid = net.store(original, DaSla::DEFAULT, 0);
        let (commitment, _returned) = net.retrieve(cid).unwrap();
        let err = verify_sla(&commitment, 0, 0, &forged);
        prop_assert_eq!(err, Err(DaError::PayloadMismatch));
    }
}
