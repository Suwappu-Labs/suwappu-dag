// Persona webhook handler for the gsx-testnet operator program.
//
// Wired by terraform/testnet/kyc.tf — receives Persona's webhook
// posts at https://kyc.testnet.gsx.globalsettlement.com/persona-webhook
// and upserts a row in the gsx_testnet_applications DynamoDB table
// keyed by the candidate's ML-DSA-65 pubkey hash.
//
// admit-operator.sh later queries this table by pubkey hash and
// gates the AdmitAuthority Intent submit on `status = "approved"`.
//
// HMAC validation is required: Persona signs each webhook body with
// HMAC-SHA-256 using a shared secret set in Persona's dashboard.
// The same secret lives in AWS Secrets Manager (gsx-testnet/kyc/persona
// → field `webhook_secret`). We compare in constant time to defeat
// timing-side-channel attacks.
//
// Why no SDK / minimal deps: this Lambda runs cold a few times a day
// (one per applicant); cold-start latency dominates over runtime
// cost, so we use only @aws-sdk/* SDKs and node:crypto.
//
// Persona payload shape (abridged, see Persona docs for the full schema):
//   {
//     "data": {
//       "type": "event",
//       "attributes": {
//         "name": "inquiry.completed" | "inquiry.approved" | "inquiry.declined" | ...,
//         "payload": {
//           "data": {
//             "type": "inquiry",
//             "id": "inq_xxx",
//             "attributes": {
//               "status": "approved" | "declined" | "needs-review" | ...,
//               "name-first": "...", "name-last": "...",
//               "email-address": "...",
//               "fields": {
//                 // Custom field we attach to the inquiry: the candidate
//                 // operator's ML-DSA pubkey hash (set during the
//                 // Typeform that embeds the Persona inquiry widget).
//                 "candidate-pubkey-hash": {"value": "0x..."},
//                 "operator-label": {"value": "acme-validator-co"}
//               }
//             }
//           }
//         }
//       }
//     }
//   }

const crypto = require("node:crypto");
const { SecretsManagerClient, GetSecretValueCommand } = require("@aws-sdk/client-secrets-manager");
const { DynamoDBClient } = require("@aws-sdk/client-dynamodb");
const { DynamoDBDocumentClient, PutCommand } = require("@aws-sdk/lib-dynamodb");

const REGION = process.env.AWS_REGION || "us-east-1";
const sm = new SecretsManagerClient({ region: REGION });
const ddb = DynamoDBDocumentClient.from(new DynamoDBClient({ region: REGION }));

// Freshness window for `persona-signature` timestamps. Wide because
// Persona's webhook retry schedule is exponential and can extend to
// ~32.7 hours (per Persona's Webhooks docs) — a tighter window would
// drop legitimate retries during outages. Replay defense is
// belt-and-suspenders: the conditional DynamoDB write below also gates
// on Persona event time (`event_created_at`), so any replayed event
// with a stale `event_created_at` is rejected at the DDB layer
// regardless of the freshness check passing.
//
// Set to 4 days — past Persona's documented max retry plus clock-skew
// headroom, still tight enough that a captured webhook is unusable
// after several days. (Codex #228 P1 follow-up — original 300s was
// too tight, blocked legitimate retries.)
const SIG_MAX_AGE_SECONDS = 4 * 24 * 60 * 60;

let cachedWebhookSecret = null;

async function loadWebhookSecret() {
  if (cachedWebhookSecret) return cachedWebhookSecret;
  const res = await sm.send(new GetSecretValueCommand({ SecretId: process.env.PERSONA_SECRET_ID }));
  const parsed = JSON.parse(res.SecretString);
  if (!parsed.webhook_secret) {
    throw new Error("PERSONA_SECRET_ID JSON missing required field `webhook_secret`");
  }
  cachedWebhookSecret = parsed.webhook_secret;
  return cachedWebhookSecret;
}

function verifySignature(rawBody, headerValue, secret) {
  // Persona's `persona-signature` header format (single set):
  //   t=<unix-ts>,v1=<hex-hmac-of-{t}.{rawBody}>,v0=<legacy>
  // During secret rotation Persona emits MULTIPLE space-separated sets,
  // each signed with a different active secret. We accept the request if
  // ANY set validates against our current secret. (Codex #228 P1 — rotation.)
  if (!headerValue) return false;
  const sets = headerValue.split(/\s+/).filter(Boolean);
  for (const set of sets) {
    if (verifyOneSignatureSet(rawBody, set, secret)) return true;
  }
  return false;
}

function verifyOneSignatureSet(rawBody, set, secret) {
  const parts = Object.fromEntries(
    set.split(",").map((p) => p.split("=").map((s) => s.trim()))
  );
  const ts = parts.t;
  const v1 = parts.v1;
  if (!ts || !v1) return false;

  // Freshness: reject sigs whose `t=` is more than SIG_MAX_AGE_SECONDS off
  // from now. Defeats captured-webhook replay (Codex #228 P2).
  const tsNum = Number.parseInt(ts, 10);
  if (!Number.isFinite(tsNum)) return false;
  const nowS = Math.floor(Date.now() / 1000);
  if (Math.abs(nowS - tsNum) > SIG_MAX_AGE_SECONDS) return false;

  const mac = crypto.createHmac("sha256", secret).update(`${ts}.${rawBody}`).digest("hex");
  if (mac.length !== v1.length) return false;
  return crypto.timingSafeEqual(Buffer.from(mac), Buffer.from(v1));
}

// Strip an optional `0x` prefix and lowercase. Persona/Typeform can deliver
// the candidate hash with or without the prefix, but admit-operator.sh
// queries DynamoDB with the bare 64-hex form, so we normalize at write
// time. (Codex #228 P2 — prefix normalization.)
function normalizePubkeyHash(raw) {
  if (typeof raw !== "string") return raw;
  return raw.toLowerCase().replace(/^0x/, "");
}

function extract(event) {
  const e = event.data?.attributes;
  if (!e) throw new Error("malformed Persona event: missing data.attributes");
  const inquiry = e.payload?.data;
  if (!inquiry || inquiry.type !== "inquiry") {
    // Persona sometimes fires non-inquiry events (account.created, etc.);
    // ignore silently.
    return null;
  }
  const attrs = inquiry.attributes || {};
  return {
    event_name: e.name,
    // Persona's event timestamp, ISO 8601. Used to gate out-of-order
    // delivery in the DynamoDB conditional write — older `needs-review`
    // events arriving after newer `approved` events must NOT overwrite
    // the row. Lambda receive time is unreliable for this because
    // network delay decorrelates it from Persona event order.
    event_created_at: e["created-at"] || null,
    inquiry_id: inquiry.id,
    status: attrs.status, // "approved" | "declined" | "needs-review" | ...
    email: attrs["email-address"] || null,
    name_first: attrs["name-first"] || null,
    name_last: attrs["name-last"] || null,
    candidate_pubkey_hash: attrs.fields?.["candidate-pubkey-hash"]?.value || null,
    operator_label: attrs.fields?.["operator-label"]?.value || null,
  };
}

exports.handler = async (event) => {
  const rawBody = event.body || "";
  const sig = event.headers?.["persona-signature"] || event.headers?.["Persona-Signature"];

  let webhookSecret;
  try {
    webhookSecret = await loadWebhookSecret();
  } catch (e) {
    console.error("failed to load webhook secret", e);
    return { statusCode: 500, body: JSON.stringify({ error: "internal" }) };
  }

  if (!verifySignature(rawBody, sig, webhookSecret)) {
    console.warn("persona-signature missing or invalid; rejecting");
    return { statusCode: 401, body: JSON.stringify({ error: "invalid signature" }) };
  }

  let payload;
  try {
    payload = JSON.parse(rawBody);
  } catch (e) {
    return { statusCode: 400, body: JSON.stringify({ error: "invalid json" }) };
  }

  const row = extract(payload);
  if (!row) {
    // Non-inquiry event; ack to stop Persona retrying.
    return { statusCode: 204, body: "" };
  }

  if (!row.candidate_pubkey_hash) {
    // The inquiry didn't carry the candidate-pubkey-hash custom field.
    // This is a misconfigured Typeform/Persona template; alarm but
    // don't 5xx (Persona would retry forever).
    console.error("inquiry missing candidate-pubkey-hash custom field", row);
    return { statusCode: 200, body: JSON.stringify({ ok: false, reason: "missing pubkey hash" }) };
  }

  if (!row.event_created_at) {
    // Persona's payload should always carry `data.attributes.created-at`;
    // a missing value would make the ordering gate below vacuous.
    // 400 (not 200) because this is a real malformed payload, not a
    // semantic edge case we want to silently ack.
    console.error("event missing data.attributes.created-at", row);
    return {
      statusCode: 400,
      body: JSON.stringify({ error: "missing data.attributes.created-at" }),
    };
  }

  const eventTime = row.event_created_at;
  const receivedAt = new Date().toISOString();
  const pkh = normalizePubkeyHash(row.candidate_pubkey_hash);
  try {
    await ddb.send(
      new PutCommand({
        TableName: process.env.DDB_TABLE,
        Item: {
          candidate_pubkey_hash: pkh,
          inquiry_id: row.inquiry_id,
          status: row.status,
          event_name: row.event_name,
          // PERSONA event time. Persisted so the next write can compare
          // against this row's actual event order, not lambda receipt
          // order. ISO 8601 strings compare chronologically.
          event_created_at: eventTime,
          // Lambda receipt time for operator observability (was
          // `updated_at`; renamed so the ordering-gate field is the
          // obvious one).
          received_at: receivedAt,
          email: row.email,
          name_first: row.name_first,
          name_last: row.name_last,
          operator_label: row.operator_label,
        },
        // Out-of-order delivery guard. Persona explicitly notes webhook
        // delivery order is not guaranteed; compare on PERSONA event
        // time (not lambda receive time — the previous revision did
        // that, which is wrong because network delay decorrelates
        // lambda time from event order, so a late-arriving older
        // event still has a *later* lambda time than the stored row
        // and silently downgrades it; Codex #228 P1 follow-up).
        // Only write if this is a new row OR the stored row's event
        // time is strictly older than this incoming event's time.
        ConditionExpression:
          "attribute_not_exists(candidate_pubkey_hash) OR event_created_at < :evt",
        ExpressionAttributeValues: { ":evt": eventTime },
      })
    );
  } catch (e) {
    // Stale event arrived after a newer one. ACK 200 so Persona doesn't
    // retry, and surface the drop in logs for alarming.
    if (e?.name === "ConditionalCheckFailedException") {
      console.warn("dropped out-of-order persona event", {
        inquiry_id: row.inquiry_id,
        status: row.status,
        event_created_at: eventTime,
        pkh,
      });
      return {
        statusCode: 200,
        body: JSON.stringify({ ok: true, dropped: "out-of-order" }),
      };
    }
    throw e;
  }

  console.log("upserted application", {
    inquiry_id: row.inquiry_id,
    status: row.status,
    event_created_at: eventTime,
    pkh,
  });

  return { statusCode: 200, body: JSON.stringify({ ok: true }) };
};
