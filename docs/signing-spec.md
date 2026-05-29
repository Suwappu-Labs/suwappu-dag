# GSX Intent Signing Specification

This document defines the canonical signing format for GSX intents.
External SDK consumers, wallets, and Solidity verifiers MUST reproduce
this exact recipe for signature verification to succeed.

## 1. Signing Digest

The signing digest is a single BLAKE3 hash over three concatenated
fields with no length prefixes:

```
digest = BLAKE3( domain_tag || network_id_bytes || intent_bincode )
```

| Field | Value | Encoding |
|---|---|---|
| `domain_tag` | `"GSX_INTENT_V1"` (14 bytes, ASCII, no null terminator) | Raw UTF-8 bytes |
| `network_id_bytes` | The network's `network_id` string (e.g. `"gsx-devnet"`, `"gsx-testnet-v1"`) | Raw UTF-8 bytes |
| `intent_bincode` | The `Intent` enum, bincode-serialized with legacy config | `bincode::serde::encode_to_vec(&intent, bincode::config::legacy())` |

**Reference implementation:** `gsx-execution/src/lib.rs`, function
`intent_signing_digest(network_id, intent_bytes)`.

### Bincode Legacy Config

The `legacy()` config uses:
- Little-endian byte order
- Fixed-size integer encoding (no varint)
- 64-bit length prefixes for variable-length fields (Vec, String)

This is the default bincode 2.x `legacy()` configuration and matches
bincode 1.x wire format.

## 2. Signature Algorithm

**ML-DSA-65** (FIPS 204, formerly Dilithium3).

- Signature size: **3,309 bytes**
- Public key size: **1,952 bytes**
- Security level: NIST Level 3 (128-bit post-quantum)

```
signature = ML-DSA-65.Sign(secret_key, digest)
```

**Reference implementation:** `gsx-crypto/src/mldsa.rs`, function
`sign(digest, secret_key)`.

## 3. Signer Identity

The signer is identified by a 32-byte hash of their ML-DSA-65 public
key:

```
signer_pubkey_hash = BLAKE3( public_key_bytes )
```

Where `public_key_bytes` is the raw 1,952-byte ML-DSA-65 public key.

### 3.1 Resolution Order

The verifier resolves `signer_pubkey_hash` using a 3-tier fallback:

1. **Authority Ring** — hash lookup against seated Authority members.
2. **Validator Ring** — hash lookup against seated Validator members.
3. **Open signer** — if the caller provides `signer_pubkey` (the raw
   ML-DSA-65 public key bytes), the verifier checks
   `BLAKE3(signer_pubkey) == signer_pubkey_hash` and uses the
   provided key for signature verification.

Tier 3 (open signer) is only accepted for **user-tier intents** such
as `Transfer`, `Delegate`, `UndelegateBegin`, `UndelegateClaim`,
`L1Lock`, `L2BurnProven`, `L2ForceInclude`, `DepositSequencerBond`,
`DepositSafetyBond`, `DepositAuthorityStake`, `DepositValidatorStake`,
`WithdrawAuthorityStake`, and `WithdrawValidatorStake`.

**Governance intents** (e.g. `AdmitAuthority`, `ExitAuthority`,
`EjectAuthority`, `GenesisAllocation`, `MintInflation`,
`CommitL2StateRoot`, `SetL2VerifyingKey`, `SlashSequencer`,
`DisburseTreasury`, etc.) require the signer to be seated in
the Authority or Validator Ring. Open signers submitting governance
intents receive `UnknownSigner`.

**Reference implementation:** `gsx-node/src/client.rs`, functions
`signer_pubkey_hash(pk_bytes)`, `intent_requires_ring_membership`,
`verify_signed_intent`.

## 4. JSON-RPC Submission

Submit a signed intent via the `gsx_submitIntent` JSON-RPC method:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "gsx_submitIntent",
  "params": {
    "intent": "0x<bincode_hex>",
    "signature": "0x<ml_dsa_signature_hex>",
    "signer_pubkey_hash": "0x<blake3_of_pubkey_hex>",
    "signer_pubkey": "0x<ml_dsa_pubkey_hex>"
  }
}
```

| Field | Required | Description |
|---|---|---|
| `intent` | Yes | Bincode-serialized `Intent`, hex-encoded |
| `signature` | Yes | ML-DSA-65 signature (3,309 bytes), hex-encoded |
| `signer_pubkey_hash` | Yes | `BLAKE3(public_key_bytes)`, 32 bytes hex |
| `signer_pubkey` | No | Raw ML-DSA-65 public key (1,952 bytes), hex-encoded. Required for open signers (tier 3). Ring members may omit — their pubkey is resolved from the registry. |

Positional array form is also accepted: `[intent, signature,
signer_pubkey_hash]` (3 elements) or `[intent, signature,
signer_pubkey_hash, signer_pubkey]` (4 elements).

All fields are hex-encoded with an optional `0x` prefix (case
insensitive). The server returns:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "tx_hash": "0x<32_byte_intent_hash_hex>"
  }
}
```

The `tx_hash` is `BLAKE3(intent_bincode)` — no domain tag.

## 5. Signing Steps (Client SDK)

1. Construct the typed `Intent` (e.g. `Intent::Transfer { from, to, amount }`).
2. Serialize with bincode legacy: `intent_bytes = bincode::serde::encode_to_vec(&intent, bincode::config::legacy())`.
3. Compute the digest: `digest = BLAKE3("GSX_INTENT_V1" || network_id || intent_bytes)`.
4. Sign: `signature = ML-DSA-65.Sign(sk, digest)`.
5. Compute signer identity: `pkh = BLAKE3(pk.as_bytes())`.
6. Submit via `gsx_submitIntent` with `intent = hex(intent_bytes)`,
   `signature = hex(signature)`, `signer_pubkey_hash = hex(pkh)`.
   - **Ring members** (Authority or Validator): omit `signer_pubkey`.
   - **Open signers** (ordinary users): include
     `signer_pubkey = hex(pk.as_bytes())` so the verifier can
     resolve the public key for signature verification.

## 6. Pinned Test Vectors

### 6.1 L2 Chain ID Hash

```
input:  "gsx-l2-chain-" || "test-chain"
recipe: BLAKE3("gsx-l2-chain-test-chain")
output: 46d743898b7c863a8fea1938f261f52134882771b3dd016999964cad793924af
```

Source: `gsx-l2-sequencer-daemon/src/batch_builder_task.rs`,
test `l2_chain_id_hash_pinned_vector`.

### 6.2 DA Commitment

```
recipe: BLAKE3(da_blob)  — no domain tag
```

The `da_commitment` field in `BatchHeader` is a plain BLAKE3 hash of
the DA blob bytes. Source: `batch_builder_task.rs`, test
`da_commitment_matches_plain_blake3`.

### 6.3 Certificate Hash

```
recipe: BLAKE3("GSX-CERT-V1" || network_id || author_4BE || round_8BE || parent_count_4BE || parents[0..n] || payload_digest)
```

`network_id` prevents cross-network replay: the same certificate content
on devnet and testnet produces different hashes.

Source: `gsx-consensus/src/cert.rs`, method `Certificate::hash(network_id)`.

### 6.4 BatchHeader Public Inputs

The `BatchHeader::to_public_inputs()` method produces exactly **240
bytes**. The Solidity verifier hard-codes this width. Source:
`batch_builder_task.rs`, test `batch_header_public_inputs_is_240_bytes`.

## 7. Address Derivation

Addresses are 20 bytes derived from the signer's ML-DSA-65 public key:

```
address = BLAKE3(public_key_bytes)[0..20]
```

The first 20 bytes of the BLAKE3 hash, matching EVM-style address
width. Source: `gsx-faucet/src/lib.rs`, function `address_from_pubkey`.
