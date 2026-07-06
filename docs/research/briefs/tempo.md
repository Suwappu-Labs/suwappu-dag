# Tempo (Stripe / Paradigm) — research brief

> Compiled 2026-07-03 from public sources (cited inline). Part of the
> competitive gap analysis — see
> [`../competitive-gap-analysis.md`](../competitive-gap-analysis.md).
> Rumored/unconfirmed items are flagged as such.

---

## 1. Current status (mid-2026)

**Mainnet is live.** Tempo launched mainnet on **March 18, 2026**, alongside the Machine Payments Protocol (MPP), an open standard for autonomous agent payments ([CoinDesk](https://www.coindesk.com/tech/2026/03/18/stripe-led-payments-blockchain-tempo-goes-live-with-protocol-for-ai-agents), [tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/), [crypto.news](https://crypto.news/stripe-and-paradigms-tempo-mainnet-goes-live-for-machine-payments/)).

Timeline of milestones:

- **Aug 2025** — Project revealed; Paradigm co-founder Matt Huang named CEO ([Fortune](https://fortune.com/crypto/2025/08/12/matt-huang-paradigm-stripe-tempo-blockchain-ceo/)).
- **Sept 2025** — Public unveiling as "the payments-first blockchain" ([tempo.xyz/blog/introducing-tempo](https://tempo.xyz/blog/introducing-tempo/)).
- **Oct 2025** — $500M Series A at $5B valuation ([Fortune](https://fortune.com/crypto/2025/10/17/stripe-paradigm-tempo-series-a-5-billion-thrive-capital-greenoaks-joshua-kushner/), [The Block](https://www.theblock.co/post/375152/stripe-tempo-500-million-series-a-thrive)).
- **Dec 9, 2025** — Public testnet launch ([The Defiant](https://thedefiant.io/news/blockchains/payments-focused-tempo-blockchain-launches-public-testnet), [BSC News](https://bsc.news/post/tempo-blockchain-testnet-launch)).
- **Mar 18, 2026** — Mainnet + MPP launch ([tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/)).
- **Apr 2026** — First external validators onboarded: Visa, Stripe, Zodia Custody (Standard Chartered) ([The Defiant](https://thedefiant.io/news/blockchains/tempo-onboards-visa-stripe-and-zodia-custody-as-validators), [Visa IR](https://investor.visa.com/news/news-details/2026/Visa-Launches-Validator-Node-on-Tempo-Blockchain/default.aspx)).
- **May 2026** — MoneyGram named "anchor remittance validator"; Stripe to settle to MoneyGram over Tempo rails ([PR Newswire](https://www.prnewswire.com/news-releases/moneygram-becomes-tempos-anchor-remittance-validator-in-strategic-blockchain-partnership-302776990.html), [The Block](https://www.theblock.co/amp/post/402043/moneygram-remittance-validator-stripe-tempo-blockchain)).
- **Jun 30, 2026** — "Open USD" (OUSD) consortium stablecoin announced with 140+ partners (Stripe, Visa, BlackRock, Coinbase, Mastercard); to be natively issued on Tempo from day one, launch expected later in 2026 ([Fortune](https://fortune.com/2026/06/30/stripe-visa-stablecoin-rival-ousd-tether-circle/), [The Block](https://www.theblock.co/post/406736/visa-stripe-coinbase-join-open-usd-stablecoin-shares-reserve-revenue)).

Next steps per Tempo: additional enterprise payment features and expanded stablecoin/payment-method support "in coming months" ([tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/)).

## 2. Technical architecture

- **Stack:** Consensus layer built on **Commonware** primitives running **Simplex BFT**; execution layer based on **Reth** ([Medium — Seungmin Jeon](https://medium.com/@organmo/tempo-architecture-analysis-2-stablecoin-gas-and-the-payment-only-lane-134f2150b9ae), [Tempo docs](https://docs.tempo.xyz/quickstart/evm-compatibility), [GitHub tempoxyz/tempo](https://github.com/tempoxyz/tempo) — open source, Apache license).
- **EVM compatibility:** Yes — Solidity, Foundry, Hardhat work; targets the **Osaka** EVM hard fork. Notable deviations: no native token (`BALANCE`/`SELFBALANCE`/`CALLVALUE` return 0 — use TIP-20 `balanceOf`); much higher state-creation gas costs (new storage slot 250k vs 20k on Ethereum; account creation 250k; ~300k gas to transfer to a fresh address) to deter state-growth attacks; "Tempo Transactions" add batching, fee sponsorship, scheduling, and explicit fee-token selection ([Tempo docs — EVM differences](https://docs.tempo.xyz/quickstart/evm-compatibility)).
- **Performance:** ~**0.5s block time**, deterministic BFT finality (~0.5–0.6s reported at mainnet); testnet ran 500 MGas/block targeting ~1 Ggas/s. Tempo markets **100,000+ TPS design target with sub-second finality**; ~20,000 TPS was benchmarked on testnet with a stated path to an order of magnitude more ([CoinDesk](https://www.coindesk.com/tech/2026/03/18/stripe-led-payments-blockchain-tempo-goes-live-with-protocol-for-ai-agents), [insights4vc](https://insights4vc.substack.com/p/tempo-stripes-blockchain-for-stablecoin), [Blockdaemon](https://www.blockdaemon.com/blog/how-tempo-works)). Treat 100k TPS as a target claim, not observed mainnet throughput.
- **Payment lanes (novel):** Protocol-level dedicated lanes reserved for TIP-20 stablecoin transfers with carved-out per-block gas budgets, so payments can't be starved or fee-spiked by general EVM activity (no "noisy neighbor" contention). Block headers carry separate limits for general execution vs. payment lanes/system sub-blocks ([Sentora/Jesus Rodriguez](https://medium.com/sentora/fine-tuning-the-world-computer-on-payments-some-technical-observations-about-tempo-1d1bfa4a70d5), [Alchemy](https://www.alchemy.com/blog/tempo-testnet-is-live-on-alchemy-the-payments-native-blockchain)).
- **Machine Payments Protocol (MPP):** Open standard (co-authored with Stripe, Visa, Lightspark) letting software/AI agents pay for APIs, data, and compute autonomously — sessions for streaming payments, a directory of 100+ integrated services; supports stablecoins, cards, and Bitcoin Lightning ([mpp.dev](https://mpp.dev/payment-methods/tempo), [tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/), [Ledger Insights](https://www.ledgerinsights.com/stripe-paradigm-launch-tempo-blockchain-alongside-machine-payments-standard/)).

## 3. Fee / gas model

- **No native gas token.** Fees are denominated in USD and paid in stablecoins (USDC, USDT, pathUSD, etc.); a cascading fee-token selection algorithm defaults to **pathUSD**, with account-level fee-token preferences ([Tempo docs](https://docs.tempo.xyz/quickstart/evm-compatibility), [seangoedecke.com FAQ](https://www.seangoedecke.com/tempo-faq/)).
- **Fee AMM:** automatically converts the payer's stablecoin into the validator's preferred stablecoin ([Alchemy](https://www.alchemy.com/blog/tempo-testnet-is-live-on-alchemy-the-payments-native-blockchain)).
- **Deterministic/fixed fee schedule** for standard payment operations rather than Ethereum-style congestion auctions — predictable, Solana-like pricing ([CoinGecko](https://www.coingecko.com/learn/what-is-tempo-stablechain), [Medium — Seungmin Jeon](https://medium.com/@organmo/tempo-architecture-analysis-2-stablecoin-gas-and-the-payment-only-lane-134f2150b9ae)).
- **Native fee sponsorship / paymaster-equivalent:** protocol separates logical sender from fee payer; a second signature can authorize paying someone else's fees, letting apps sponsor user gas ([Tempo docs — Sponsor User Fees](https://docs.tempo.xyz/guide/payments/sponsor-user-fees), [Sentora](https://medium.com/sentora/fine-tuning-the-world-computer-on-payments-some-technical-observations-about-tempo-1d1bfa4a70d5)). Account abstraction is first-class in the execution layer ([Medium — Tempo AA analysis](https://medium.com/@organmo/tempo-architecture-analysis-1-tempos-account-abstraction-6babdeabc93e)).

## 4. Stablecoin & payments features

- **TIP-20 token standard** for stablecoins; issuance is native to the chain ([Tempo docs](https://docs.tempo.xyz/quickstart/evm-compatibility)).
- **pathUSD** — Tempo's native/default gas stablecoin ([Alain AI Lab](https://intelligencecrypto.org/reports/what-is-stripes-tempo-stablecoin)); USDC and USDT usable for fees.
- **Open USD (OUSD)** — consortium stablecoin (Open Standard entity) with 140+ partners incl. Stripe, Visa, Mastercard, BlackRock, Coinbase; most reserve yield shared with participants; free mint/redeem; natively issued on Tempo from day one; launch expected H2 2026. Tether and Circle are notably absent ([Fortune](https://fortune.com/2026/06/30/stripe-visa-stablecoin-rival-ousd-tether-circle/), [The Block](https://www.theblock.co/post/406736/visa-stripe-coinbase-join-open-usd-stablecoin-shares-reserve-revenue)). Circle's stock fell ~16% on the news ([Coingabbar](https://www.coingabbar.com/en/crypto-currency-news/open-standard-new-open-usd-ousd-stablecoin-launch-2026)).
- **Multi-currency/FX:** mainnet materials reference non-USD stablecoins (e.g., a Swiss franc stablecoin "CHFAU"), and the Fee AMM provides protocol-level stablecoin conversion; built-in FX was part of the original pitch ([tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/), [tempo.xyz](https://tempo.xyz/)).
- **Privacy:** opt-in confidential balances/transfers — "confidential transactions with the auditability compliance requires" ([tempo.xyz](https://tempo.xyz/)).
- **Compliance hooks:** TIP-403 Policy Registry lets issuers attach transfer policies (sanctions screening, allow/deny lists) at the protocol level ([Sentora](https://medium.com/sentora/fine-tuning-the-world-computer-on-payments-some-technical-observations-about-tempo-1d1bfa4a70d5)); memo fields for payment reconciliation are native ([CoinDesk](https://www.coindesk.com/tech/2026/03/18/stripe-led-payments-blockchain-tempo-goes-live-with-protocol-for-ai-agents)).

## 5. Validator model / decentralization

- **Permissioned at launch**, with a stated roadmap to permissionless. Testnet began with ~4 company-run validators; the plan is to expand through design partners first ([DL News](https://www.dlnews.com/articles/markets/stripe-backed-tempo-blockchain-launches-public-testnet/), [tempo.xyz/blog/introducing-tempo](https://tempo.xyz/blog/introducing-tempo/)).
- **External validators (2026):** Visa (own node live April 2026), Stripe, Zodia Custody/Standard Chartered ([The Defiant](https://thedefiant.io/news/blockchains/tempo-onboards-visa-stripe-and-zodia-custody-as-validators), [Visa IR](https://investor.visa.com/news/news-details/2026/Visa-Launches-Validator-Node-on-Tempo-Blockchain/default.aspx), [PYMNTS](https://www.pymnts.com/blockchain/2026/visa-deepens-blockchain-involvement-with-tempo-network-validator/)); MoneyGram as anchor remittance validator (May 2026) ([PR Newswire](https://www.prnewswire.com/news-releases/moneygram-becomes-tempos-anchor-remittance-validator-in-strategic-blockchain-partnership-302776990.html)). An exact current validator count is not published; it remains a small, curated institutional set.
- Simplex BFT + permissioned set is an explicit trade: institutional speed, compliance control, and guaranteed finality over near-term decentralization ([Techopedia](https://www.techopedia.com/tempo-stablecoin-payments-neutrality)).

## 6. Ecosystem & partners

- **Design partners:** Visa, Mastercard, Deutsche Bank, Standard Chartered, Revolut, Nubank, Shopify, OpenAI, Anthropic, Ramp, DoorDash, Lightspark ([CoinDesk](https://www.coindesk.com/tech/2026/03/18/stripe-led-payments-blockchain-tempo-goes-live-with-protocol-for-ai-agents), [Ledger Insights](https://www.ledgerinsights.com/stripe-paradigm-launch-tempo-blockchain-alongside-machine-payments-standard/)).
- **Tooling:** standard EVM toolchain (Foundry/Hardhat/Solidity); RPC via Alchemy (with support for TIP-20, TempoTransactions, payment lanes), QuickNode (multi-region, archive), Chainstack and others ([Alchemy](https://www.alchemy.com/rpc/tempo), [QuickNode](https://www.quicknode.com/chains/tempo), [Chainstack](https://chainstack.com/best-tempo-rpc-providers/)); public RPC endpoints since mainnet ([tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/)).
- **Explorer:** official explorer at explore.tempo.xyz ([Tempo docs — developer tools](https://docs.tempo.xyz/quickstart/developer-tools)).
- **MPP directory:** 100+ services integrated for agent payments at launch ([tempo.xyz/blog/mainnet](https://tempo.xyz/blog/mainnet/)).
- **Node source:** open source at [github.com/tempoxyz/tempo](https://github.com/tempoxyz/tempo).

## 7. Team, funding, valuation

- **CEO:** Matt Huang (Paradigm co-founder/managing partner, Stripe board member; remains at Paradigm) ([Fortune](https://fortune.com/crypto/2025/08/12/matt-huang-paradigm-stripe-tempo-blockchain-ceo/)).
- **Notable hires:** Ethereum Foundation researcher **Dankrad Feist** joined Tempo (a controversial departure for Ethereum) ([Blockworks](https://blockworks.com/news/dankrad-feist-move-tempo)); Farcaster co-founders **Dan Romero and Varun Srinivasan** plus much of the Merkle team joined in Feb 2026 after Neynar acquired the Farcaster protocol ([CoinDesk](https://www.coindesk.com/business/2026/02/09/farcaster-founders-join-stablecoin-startup-tempo-after-neynar-acquires-social-protocol), [The Block](https://www.theblock.co/post/389076/farcaster-founders-dan-romero-and-varun-srinivasan-join-stablecoin-startup-tempo)).
- **Funding:** **$500M Series A (Oct 2025) at a $5B valuation**, led by Greenoaks and Thrive Capital, with Sequoia, Ribbit, and SV Angel participating; Stripe and Paradigm did not contribute capital to that round ([Fortune](https://fortune.com/crypto/2025/10/17/stripe-paradigm-tempo-series-a-5-billion-thrive-capital-greenoaks-joshua-kushner/), [Blockworks](https://blockworks.com/news/tempo-series-a-raise)).
- No native token exists; monetization is not via token appreciation. (Any "Tempo token" chatter is unconfirmed speculation.)

## 8. Go-to-market positioning & criticisms

**Pitch:** neutral, purpose-built rails for real-world stablecoin payments at internet scale — payouts, remittances, B2B settlement, and (increasingly, the 2026 emphasis) **AI-agent/machine payments** via MPP. Target customers are enterprises, fintechs, banks, and AI platforms rather than retail crypto users; positioning stresses predictable USD fees, sub-second finality, compliance hooks, and no volatile gas asset ([tempo.xyz](https://tempo.xyz/), [Ledger Insights](https://www.ledgerinsights.com/stripe-paradigm-launch-tempo-blockchain-alongside-machine-payments-standard/), [Blockworks](https://blockworks.com/news/tempo-stripe-paradigm-planned-l1)).

**Criticisms:**

- **Credible neutrality doubts.** Libra architect Christian Catalini: "Would a sane competitor bet its future on Stripe's promise not to eventually favor its own products?" Agora's Nick van Eck called the neutrality framing "gaslighting," reading Tempo as Stripe "declaring war" on stablecoin incumbents, card networks, and banks ([BeInCrypto](https://beincrypto.com/stripes-tempo-blockchain-the-new-libra-or-ethereum-killer/)).
- **Centralization.** Permissioned BFT validator set run by corporates draws "new Libra" comparisons and questions about censorship and governance capture ([Techopedia](https://www.techopedia.com/tempo-stablecoin-payments-neutrality)).
- **Crowded field.** Competes with Tron, Solana, Ethereum L2s, plus rival payment chains — Circle's Arc and Tether/Bitfinex-backed Stable ([PayRam comparison](https://www.payram.com/blog/tempo-vs-stable-vs-arc)). The OUSD consortium sharpens the direct conflict with Circle/Tether ([Fortune](https://fortune.com/2026/06/30/stripe-visa-stablecoin-rival-ousd-tether-circle/)).
- **Ethereum-community friction** over talent drain (Feist) and value migrating to a corporate L1 outside credibly neutral infrastructure ([Blockworks](https://blockworks.com/news/dankrad-feist-move-tempo)).
- **Claims vs. reality gap:** 100k+ TPS is a design target; observed benchmarks were ~20k TPS on testnet, and real mainnet payment volume figures have not been publicly disclosed as of early July 2026.
