# IQ-7 Anchor Auth — Hybrid AND-gate

Landed by `gsx-db` PR #4 (anchor/iq7-scheme-alignment).

## AuthScheme + credential envelope

```mermaid
flowchart LR
    subgraph AS["AuthScheme  (#[repr(u8)])"]
        Blake3["Blake3Mac  = 0"]
        Sp1["Sp1ZkProof = 1"]
        Ecdsa["EcdsaSecp256k1 = 2"]
        Hybrid["MlDsa65Hybrid  = 3"]
    end

    subgraph CRED["AnchorAuthCredential"]
        cBlake["::Blake3 { mac }"]
        cSp1["::Sp1 { vkey_hash, public_values, proof_bytes }"]
        cEcdsa["::Ecdsa { sig: [u8;65] }"]
        cHybrid["::Hybrid { ecdsa_sig, mldsa_sig }"]
    end

    Blake3 --> cBlake
    Sp1    --> cSp1
    Ecdsa  --> cEcdsa
    Hybrid --> cHybrid
```

## verify_credential dispatch

```mermaid
flowchart TD
    Start([verify_credential<br/>anchor, credential, expected]) --> Match{credential variant}

    Match -- Blake3 --> B["verify_mac<br/>(unchanged from S7)"]
    Match -- Sp1 --> Sp1Pre["pre-check:<br/>vkey_hash match?<br/>public_values match?"]
    Match -- Ecdsa --> EcdsaPath
    Match -- Hybrid --> HybridPath

    Sp1Pre -- mismatch --> Sp1Err[/"Sp1VkeyMismatch /<br/>Sp1PublicValuesMismatch"/]
    Sp1Pre -- pass --> Sp1Stub[/"UnsupportedScheme<br/>(zkVM verify deferred —<br/>Track 1.3 Step 2)"/]

    subgraph EcdsaPath["verify_ecdsa  (Solidity-parity)"]
        E1["abi.encode(anchor)<br/>5 × 32 bytes"] --> E2["keccak256"]
        E2 --> E3["EIP-191:<br/>'\x19Ethereum Signed Message:\n32' || inner"]
        E3 --> E4["VerifyingKey::recover_from_prehash"]
        E4 --> E5{recovered ==<br/>expected_signer?}
        E5 -- yes --> Eok[Ok]
        E5 -- no  --> Eerr[UnauthorizedSigner]
    end

    subgraph HybridPath["verify_hybrid  (AND-gate)"]
        H1[verify_ecdsa half] --> H1ok{ok?}
        H1ok -- no  --> Hecdsa[EcdsaFailed]
        H1ok -- yes --> H2[verify_mldsa65 half<br/>shared EIP-191 payload]
        H2 --> H2ok{ok?}
        H2ok -- no  --> Hmldsa[MlDsaFailed]
        H2ok -- yes --> Hok[Ok]
    end

    classDef err fill:#7f1d1d,stroke:#991b1b,color:#fee2e2
    classDef defer fill:#78350f,stroke:#92400e,color:#fef3c7
    class Sp1Err,Eerr,Hecdsa,Hmldsa err
    class Sp1Stub defer
```

## Parity-critical invariants

- **ECDSA payload** is byte-exact vs. `LTPAnchorRegistry.recoverSigner` —
  same `abi.encode` field layout, same EIP-191 prefix bytes, same
  `ecrecover` semantics.
- **Low-s** signatures rejected on both sides (EIP-2 malleability) —
  Rust via `Signature::normalize_s().is_some()`, Solidity via
  `uint256(s) <= secp256k1n/2`.
- **v-byte strict** on both sides — only `{27, 28}` accepted (no
  silent `v += 27` bump).
- **`production-pqc` feature off** → ML-DSA verifier is a stub that
  returns `UnsupportedScheme`. Hybrid credential cannot accidentally
  validate without the PQ build.
- **Sp1 pre-check vs verify** — even when structural pre-checks pass,
  the arm returns `UnsupportedScheme(Sp1ZkProof)`. Pre-check success
  is never mistaken for full verification.

## Discriminant table

| Variant            | u8 |
|--------------------|----|
| `Blake3Mac`        | 0  |
| `Sp1ZkProof`       | 1  |
| `EcdsaSecp256k1`   | 2  (reuses the old `PostQuantumSig` slot) |
| `MlDsa65Hybrid`    | 3  (new) |

Pinned by `auth_scheme_discriminants_are_stable` in
`crates/gsxdb-bridge/src/anchor/types.rs`. Renaming a variant or
reordering would break wire-format compatibility with serialized
anchors.

## Outstanding follow-ups (knowingly deferred)

| Item | Tracked in | When |
|---|---|---|
| `AnchorDispatcher` wiring for ECDSA / Hybrid | `types.rs` doc comments | Track 1.2 Steps B-D, S11 |
| Sp1 cryptographic proof verification | `credential.rs:418` | Track 1.3 Step 2 (zkVM toolchain decision) |
| 36-pair conformance matrix coverage of new variants | `docs/architecture/sprint-execution-checklist.md` | S11 exit gate |
| `Anchor::hash` includes `auth_scheme` byte; Solidity `hashAnchor` does not | parity-checker review | S11 (paired ABI fix) |
