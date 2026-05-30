<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Apply to run a gsx-testnet validator</title>
  <meta name="description" content="Apply to operate a validator on the GSX DAG L1 incentivized public testnet. Points earned during testnet convert to mainnet token at TGE.">
  <style>
    * { box-sizing: border-box; }
    body {
      margin: 0; padding: 0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
      background: #0e1116;
      color: #d9dee3;
      min-height: 100vh;
      display: flex; flex-direction: column;
    }
    header {
      padding: 1.5rem 2rem;
      border-bottom: 1px solid #1f242b;
      display: flex; align-items: baseline; gap: 1rem;
    }
    header h1 { margin: 0; font-size: 1.2rem; font-weight: 600; }
    header .tag { font-size: 0.8rem; color: #6b7785; text-transform: uppercase; letter-spacing: 0.08em; }
    main { flex: 1; display: flex; flex-direction: column; }
    .intro {
      max-width: 720px;
      margin: 2rem auto 1rem;
      padding: 0 1.5rem;
      font-size: 1rem;
      line-height: 1.55;
    }
    .intro a { color: #7cb8ff; }
    .form-wrap {
      flex: 1;
      width: 100%;
      max-width: 960px;
      margin: 0 auto;
      padding: 0 1.5rem 2rem;
    }
    iframe {
      width: 100%;
      height: 720px;
      border: 0;
      border-radius: 8px;
      background: #161b22;
    }
    footer {
      padding: 1rem 2rem;
      border-top: 1px solid #1f242b;
      font-size: 0.8rem;
      color: #6b7785;
      text-align: center;
    }
    footer a { color: #6b7785; }
  </style>
</head>
<body>
  <header>
    <h1>gsx-testnet — validator operator program</h1>
    <span class="tag">Apply</span>
  </header>
  <main>
    <div class="intro">
      <p>
        External validator operators run gsx-node on their own hardware,
        peer with the foundation's 7 seed regions, and earn points that
        convert to mainnet token at TGE (capped at <strong>5–8% of mainnet
        supply</strong>; allocation pro-rata to total points). Before
        applying, review the hardware spec and the points formula in
        <a href="https://github.com/GlobalSettlementNetwork/gsx-dag/blob/main/docs/testnet/VALIDATOR-OPERATORS.md">VALIDATOR-OPERATORS.md</a>
        and <a href="https://github.com/GlobalSettlementNetwork/gsx-dag/blob/main/docs/testnet/POINTS.md">POINTS.md</a>.
      </p>
      <p>
        The form below includes an in-line KYC step (powered by Persona)
        — a government ID + selfie capture is required to comply with
        the foundation's token-distribution policy. Foundation operations
        review approved applications within ~5 business days and contact
        you over the email you provide.
      </p>
    </div>
    <div class="form-wrap">
      <iframe
        src="${typeform_url}"
        title="gsx-testnet operator application"
        loading="lazy"
        referrerpolicy="strict-origin-when-cross-origin"
        allow="camera; microphone; clipboard-write"
      ></iframe>
    </div>
  </main>
  <footer>
    <a href="https://github.com/GlobalSettlementNetwork/gsx-dag">gsx-dag</a>
    · <a href="https://github.com/GlobalSettlementNetwork/gsx-dag/blob/main/docs/testnet/VALIDATOR-OPERATORS.md">operator guide</a>
    · <a href="https://github.com/GlobalSettlementNetwork/gsx-dag/blob/main/docs/testnet/POINTS.md">points formula</a>
    · <a href="https://github.com/GlobalSettlementNetwork/gsx-dag/blob/main/SECURITY.md">security</a>
  </footer>
</body>
</html>
