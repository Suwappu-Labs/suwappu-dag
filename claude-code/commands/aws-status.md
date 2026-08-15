---
description: Snapshot AWS infra health (read-only)
---

Produce a read-only health snapshot of the devnet/testnet infrastructure
(AWS profile `gsn`, account 492042618949, us-east-1):

1. `aws ec2 describe-instances` — validator/node instance states.
2. `aws cloudwatch describe-alarms --state-value ALARM` — firing alarms.
3. Probe the public RPC/status endpoints listed in `DEVNET.md` and
   `OPERATIONS.md` with curl.
4. Summarize: healthy / degraded / down per component, with the evidence
   for each claim.

Read-only means read-only: no mutations, no restarts, no deploys from
this command. If something is broken, report it and propose the fix as a
separate, explicitly-confirmed action (deploys go through
`./scripts/deploy-aws.sh`, never raw terraform).
