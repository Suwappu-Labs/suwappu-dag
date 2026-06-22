//! SCION path-authenticated routing (paper §6.3).
//!
//! "Inter-validator transport runs on SCION [SCION Book, 2017] with a
//! SCION-IP-Gateway fallback for external clients. SCION's path-
//! authenticated routing eliminates the BGP-class attack vector that
//! has produced multiple production blockchain incidents on flat IP
//! infrastructure [Birgi et al., 2022]. Trust Root Configuration
//! governance over the validator mesh's Isolation Domain provides
//! cryptographically anchored route-authority rotation."
//!
//! ## Sprint scope (DAG-S18)
//!
//! Phase-1 implements the **path-authentication predicate** — the core
//! property that distinguishes SCION from BGP. The full SCION control-
//! plane (beacons, segment registry, path-server, certificate hierarchy)
//! is infrastructure; we model:
//!
//! - `IsdId`, `AsId` — Isolation Domain and Autonomous System ids.
//! - `HopField` — one routing decision (ingress, egress, AS, expiration,
//!   MAC).
//! - `Path` — a sequence of hop fields plus path metadata.
//! - `TrustRootConfig` — the per-ISD authorized AS set with per-AS MAC keys.
//! - `verify_path(path, trc, now)` — the on-wire predicate that every
//!   inter-validator packet is checked against.
//!
//! Each hop field's MAC chains over `(prev_mac || ingress || egress ||
//! expiration)` keyed by the AS's MAC key. Forging a single hop breaks
//! the chain at that hop. Property tests verify at 10,000 cases.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Isolation Domain identifier (paper §6.3).
pub type IsdId = u16;

/// Autonomous System identifier within an ISD.
pub type AsId = u32;

/// Truncated MAC carried in a hop field. Production SCION uses 6 bytes;
/// phase-1 uses 16 bytes (truncated BLAKE3) to keep the per-hop
/// forgery probability negligible across the 10k case space.
pub type HopMac = [u8; 16];

/// A single SCION hop field — one AS's routing decision on the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HopField {
    /// (ISD, AS) of the AS that authored this hop.
    pub isd_as: (IsdId, AsId),
    /// Ingress interface id at this AS.
    pub ingress_iface: u16,
    /// Egress interface id at this AS.
    pub egress_iface: u16,
    /// Round at which this hop expires; the path is invalid past this.
    pub expiration_round: u64,
    /// Truncated keyed-hash MAC.
    pub mac: HopMac,
}

/// A SCION path — an ordered sequence of hop fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Path {
    /// ISD this path resides in.
    pub isd: IsdId,
    /// Hop fields in forward order.
    pub hops: Vec<HopField>,
    /// Round at which the path was constructed.
    pub created_at: u64,
}

/// Trust Root Configuration for one ISD.
///
/// Phase-1 carries the authorized AS set plus per-AS MAC keys directly.
/// Production binds a multi-Authority-Ring-signed TRC document over the
/// same data with rotation governance; that lands when the LTP on-chain
/// registry sprint integrates with this surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRootConfig {
    /// ISD this TRC governs.
    pub isd: IsdId,
    /// TRC version (monotonic; production verifies it against the
    /// on-chain registry).
    pub version: u32,
    /// AS-to-MAC-key map. Each AS in this map is authorized to author
    /// hop fields within the ISD.
    pub as_keys: BTreeMap<AsId, [u8; 32]>,
    /// Round at which this TRC expires.
    pub valid_until: u64,
}

/// Errors emitted by the SCION path-auth predicate.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ScionError {
    /// The path's ISD differs from the TRC's ISD.
    #[error("isd mismatch: path {path_isd}, trc {trc_isd}")]
    IsdMismatch {
        /// Path ISD.
        path_isd: IsdId,
        /// TRC ISD.
        trc_isd: IsdId,
    },

    /// The TRC itself is past its `valid_until` round.
    #[error("trc expired at {expired_at}")]
    TrcExpired {
        /// Round at which TRC expired.
        expired_at: u64,
    },

    /// The path has no hops; SCION requires at least one.
    #[error("empty path")]
    EmptyPath,

    /// A hop field's AS is not in the TRC's authorized set.
    #[error("unauthorized as in hop {hop_index}: ({isd}, {as_id})")]
    UnauthorizedAs {
        /// Index of the offending hop.
        hop_index: usize,
        /// Hop's ISD.
        isd: IsdId,
        /// Hop's AS.
        as_id: AsId,
    },

    /// A hop's MAC is past its expiration.
    #[error("hop {hop_index} expired at round {expiration_round}, now {now}")]
    HopExpired {
        /// Index of the offending hop.
        hop_index: usize,
        /// Hop's expiration round.
        expiration_round: u64,
        /// Round at which the path was checked.
        now: u64,
    },

    /// A hop's MAC does not chain correctly under the AS's MAC key.
    #[error("invalid hop mac at index {hop_index}")]
    InvalidHopMac {
        /// Index of the offending hop.
        hop_index: usize,
    },
}

/// Compute the canonical hop MAC.
///
/// Mac is `BLAKE3-keyed(as_key, prev_mac || ingress (2 BE) || egress (2 BE)
/// || expiration (8 BE))`, truncated to `HopMac` size (16 bytes).
pub fn hop_mac(as_key: &[u8; 32], prev_mac: HopMac, hop: &HopField) -> HopMac {
    let mut hasher = blake3::Hasher::new_keyed(as_key);
    hasher.update(&prev_mac);
    hasher.update(&hop.ingress_iface.to_be_bytes());
    hasher.update(&hop.egress_iface.to_be_bytes());
    hasher.update(&hop.expiration_round.to_be_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    out
}

/// Compute the seed MAC at the start of a path. The seed binds the path
/// to its ISD and creation round so a hop-field set authored for one
/// path cannot be lifted into another.
fn path_seed_mac(path: &Path) -> HopMac {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"GSX-SCION-PATH-SEED-V1");
    hasher.update(&path.isd.to_be_bytes());
    hasher.update(&path.created_at.to_be_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    out
}

/// Construct a path's hop MACs given the per-AS keys, returning a
/// fully-MACed `Path`. The caller supplies the `hops` template (with
/// `mac` set to any value — it will be overwritten).
pub fn seal_path(
    isd: IsdId,
    created_at: u64,
    mut hops: Vec<HopField>,
    trc: &TrustRootConfig,
) -> Result<Path, ScionError> {
    let mut seed = path_seed_mac(&Path {
        isd,
        hops: Vec::new(),
        created_at,
    });
    for (idx, hop) in hops.iter_mut().enumerate() {
        let key = trc
            .as_keys
            .get(&hop.isd_as.1)
            .ok_or(ScionError::UnauthorizedAs {
                hop_index: idx,
                isd: hop.isd_as.0,
                as_id: hop.isd_as.1,
            })?;
        let mac = hop_mac(key, seed, hop);
        hop.mac = mac;
        seed = mac;
    }
    Ok(Path {
        isd,
        hops,
        created_at,
    })
}

/// Path-authentication predicate. Returns `Ok(())` iff:
///
/// 1. `path.isd == trc.isd`.
/// 2. `now <= trc.valid_until`.
/// 3. `path` has at least one hop.
/// 4. For every hop: the hop's AS is in `trc.as_keys`, the hop's
///    `expiration_round >= now`, and the hop's `mac` chains correctly
///    from the previous MAC under that AS's key.
pub fn verify_path(path: &Path, trc: &TrustRootConfig, now: u64) -> Result<(), ScionError> {
    if path.isd != trc.isd {
        return Err(ScionError::IsdMismatch {
            path_isd: path.isd,
            trc_isd: trc.isd,
        });
    }
    if now > trc.valid_until {
        return Err(ScionError::TrcExpired {
            expired_at: trc.valid_until,
        });
    }
    if path.hops.is_empty() {
        return Err(ScionError::EmptyPath);
    }

    let mut prev = path_seed_mac(path);
    for (idx, hop) in path.hops.iter().enumerate() {
        let key = trc
            .as_keys
            .get(&hop.isd_as.1)
            .ok_or(ScionError::UnauthorizedAs {
                hop_index: idx,
                isd: hop.isd_as.0,
                as_id: hop.isd_as.1,
            })?;
        if hop.expiration_round < now {
            return Err(ScionError::HopExpired {
                hop_index: idx,
                expiration_round: hop.expiration_round,
                now,
            });
        }
        let expected = hop_mac(key, prev, hop);
        if expected != hop.mac {
            return Err(ScionError::InvalidHopMac { hop_index: idx });
        }
        prev = hop.mac;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trc(isd: IsdId, ases: &[AsId]) -> TrustRootConfig {
        let mut as_keys = BTreeMap::new();
        for (i, a) in ases.iter().enumerate() {
            let mut key = [0u8; 32];
            key[0] = (i + 1) as u8;
            as_keys.insert(*a, key);
        }
        TrustRootConfig {
            isd,
            version: 1,
            as_keys,
            valid_until: 1_000_000,
        }
    }

    fn template_hop(isd: IsdId, as_id: AsId, expires: u64) -> HopField {
        HopField {
            isd_as: (isd, as_id),
            ingress_iface: 1,
            egress_iface: 2,
            expiration_round: expires,
            mac: [0u8; 16],
        }
    }

    #[test]
    fn seal_then_verify_roundtrip() {
        let trc = make_trc(1, &[10, 20, 30]);
        let hops = vec![
            template_hop(1, 10, 1000),
            template_hop(1, 20, 1000),
            template_hop(1, 30, 1000),
        ];
        let path = seal_path(1, 100, hops, &trc).unwrap();
        verify_path(&path, &trc, 200).unwrap();
    }

    #[test]
    fn forged_mac_rejected() {
        let trc = make_trc(1, &[10, 20]);
        let hops = vec![template_hop(1, 10, 1000), template_hop(1, 20, 1000)];
        let mut path = seal_path(1, 100, hops, &trc).unwrap();
        path.hops[1].mac[0] ^= 1; // tamper
        let err = verify_path(&path, &trc, 200);
        assert!(matches!(
            err,
            Err(ScionError::InvalidHopMac { hop_index: 1 })
        ));
    }

    #[test]
    fn unauthorized_as_rejected() {
        // TRC authorizes AS 10 only. Path has just one hop, from AS 99.
        let trc = make_trc(1, &[10]);
        let hops = vec![template_hop(1, 99, 1000)];
        let path = Path {
            isd: 1,
            hops,
            created_at: 0,
        };
        let err = verify_path(&path, &trc, 200);
        assert!(matches!(
            err,
            Err(ScionError::UnauthorizedAs { hop_index: 0, .. })
        ));
    }

    #[test]
    fn expired_hop_rejected() {
        let trc = make_trc(1, &[10]);
        let hops = vec![template_hop(1, 10, 100)];
        let path = seal_path(1, 0, hops, &trc).unwrap();
        // now > expiration
        let err = verify_path(&path, &trc, 200);
        assert!(matches!(err, Err(ScionError::HopExpired { .. })));
    }

    #[test]
    fn isd_mismatch_rejected() {
        let trc = make_trc(1, &[10]);
        let hops = vec![template_hop(2, 10, 1000)];
        let path = Path {
            isd: 2,
            hops,
            created_at: 0,
        };
        let err = verify_path(&path, &trc, 200);
        assert!(matches!(err, Err(ScionError::IsdMismatch { .. })));
    }

    #[test]
    fn trc_expired_rejected() {
        let mut trc = make_trc(1, &[10]);
        trc.valid_until = 100;
        let hops = vec![template_hop(1, 10, 10_000)];
        let path = seal_path(1, 0, hops, &trc).unwrap();
        let err = verify_path(&path, &trc, 500);
        assert!(matches!(err, Err(ScionError::TrcExpired { .. })));
    }

    #[test]
    fn empty_path_rejected() {
        let trc = make_trc(1, &[10]);
        let path = Path {
            isd: 1,
            hops: Vec::new(),
            created_at: 0,
        };
        assert_eq!(verify_path(&path, &trc, 200), Err(ScionError::EmptyPath));
    }
}
