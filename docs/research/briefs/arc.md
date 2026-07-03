# Arc (Circle) — research brief

> Compiled 2026-07-03 from public sources (cited inline). Part of the
> competitive gap analysis — see
> [`../competitive-gap-analysis.md`](../competitive-gap-analysis.md).
> Rumored/unconfirmed items are flagged as such.

---

## 1. Current status (mid-2026)

**State: public testnet live; mainnet not yet launched. Mainnet (beta) targeted for summer 2026.**

- **Aug 12, 2025** — Arc announced as "an open Layer-1 blockchain purpose-built for stablecoin finance," with a private testnet in the following weeks and public testnet in fall 2025 ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).
- **Oct 27–28, 2025** — Public testnet launched with 100+ institutional participants ([Circle pressroom](https://www.circle.com/pressroom/circle-launches-arc-public-testnet); [CoinDesk](https://www.coindesk.com/business/2025/10/27/circle-issuer-of-usdc-starts-testing-arc-blockchain-with-big-institutions-onboard)).
- **Feb 2026** — Testnet had processed 166M+ transactions with ~half-second finality and near-perfect uptime (Circle-reported; via [arc.network blog](https://www.arc.network/blog/meet-the-arc-track-winners-from-the-hackmoney-2026-hackathon-and-what-we-learned) / Circle materials).
- **Apr 2026** — Circle announced Arc will be **quantum-resistant at mainnet launch**, adding a post-quantum signature scheme for optional quantum-resistant wallets ([Arc blog on PQ roadmap](https://www.arc.io/blog/arcs-quantum-resistant-design-and-roadmap-why-it-matters)).
- **May 11, 2026** — **ARC token whitepaper** published; mainnet launch stated for **summer 2026**; testnet reported ~244M cumulative transactions by early May ([Arc blog](https://www.arc.io/blog/introducing-the-arc-token-whitepaper); [Phemex News](https://phemex.com/news/article/circle-unveils-arc-blockchain-whitepaper-mainnet-launch-set-for-summer-2026-82817); [KuCoin News](https://www.kucoin.com/news/articles/circle-ceo-confirms-arc-token-exploration-mainnet-launch-set-for-2026)).
- **May 2026** — Circle disclosed a **$222M private presale of ARC at a $3B fully-diluted valuation** alongside Q1 2026 results — reportedly the first token presale by a publicly listed US company ([The Block](https://www.theblock.co/post/400709/circle-raises-222m-in-arc-token-presale-at-3b-fdv-from-a16z-crypto-blackrock-and-others-q1-revenue-up-20); [Yahoo Finance](https://finance.yahoo.com/markets/crypto/articles/circle-raises-222m-arc-token-123150779.html); [StockTitan 8-K summary](https://www.stocktitan.net/sec-filings/CRCL/8-k-circle-internet-group-inc-reports-material-event-a080242d188f.html)). Circle also launched a **cirBTC testnet** (wrapped Bitcoin on Arc) in May ([The Crypto Times](https://www.cryptotimes.io/2026/05/22/circle-launches-cirbtc-testnet-ahead-of-wrapped-bitcoin-rollout/)).
- **Jun 18, 2026** — Testnet **v0.7.2 upgrade** activated, widely read as pre-mainnet hardening ([Coin Gabbar](https://www.coingabbar.com/en/crypto-currency-news/arc-testnet-upgrade-v0-7-2-activation-date-june-2026); [MEXC News](https://www.mexc.com/news/1144042)).
- **Testnet activity (week of Jun 18–24, 2026, per [arc.io](https://www.arc.io/))**: ~23.4M weekly transactions, ~90K weekly new accounts, ~1.7M weekly contract deployments, average tx cost ~$0.009.
- As of early July 2026, **no confirmed mainnet launch date** has been announced; CEO Jeremy Allaire has said Circle "hopes to go to mainnet soon" ([Decrypt](https://decrypt.co/364295/circle-exploring-arc-network-token-proof-stake-shift-ceo)).

## 2. Technical architecture

- **Consensus: Malachite** — a BFT consensus engine based on the **Tendermint** algorithm, originally built by Informal Systems; the Malachite team and IP joined Circle to build Arc ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance); [Sentora technical notes](https://medium.com/sentora/some-technical-notes-about-circles-new-blockchain-d09b8d26e0a4)). Deterministic (irreversible) finality — no probabilistic reorgs.
- **Execution: EVM-compatible**, so standard Ethereum tooling/frameworks work unmodified ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).
- **Performance claims**: **sub-second deterministic finality** (~350 ms in tests); **~3,000 TPS demonstrated with 20 geographically distributed validators**, with claims it is theoretically capable of 10k+ TPS ([Sentora](https://medium.com/sentora/some-technical-notes-about-circles-new-blockchain-d09b8d26e0a4); [Coin Bureau](https://coinbureau.com/education/what-is-arc-circle-stablechain)). Testnet has run at ~0.5 s finality ([Circle](https://www.circle.com/pressroom/circle-launches-arc-public-testnet)).
- **Blockspace design**: positioned as "institutional/enterprise-grade blockspace" with predictable capacity for payments/settlement; fee smoothing (below) is the main published blockspace-economics mechanism ([arc.io](https://www.arc.io/); [Imperator overview](https://www.imperator.co/resources/blog/what-is-arc-blockchain-circle%E2%80%99s-stablecoin-native-l1)).
- **Post-quantum roadmap**: optional PQ-signature wallets at mainnet, with a longer-term quantum-resistance program ([Arc blog](https://www.arc.io/blog/arcs-quantum-resistant-design-and-roadmap-why-it-matters)).
- **Open source**: core software to be released under a permissive license ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).

## 3. Fee / gas model

- **USDC is the native gas token** — fees are dollar-denominated; no volatile asset needed ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).
- **Modified EIP-1559**: instead of per-block base-fee recalculation, Arc uses an **exponentially weighted moving average (EWMA) of block utilization** plus a **base-fee ceiling**, so fees adjust gradually and stay predictable under demand spikes ([Coin Bureau](https://coinbureau.com/education/what-is-arc-circle-stablechain); [PayRam guide](https://www.payram.com/blog/what-is-arc-blockchain-the-definitive-guide-to-circles-stablecoin-superhighway)).
- Fees are directed to an **on-chain Arc Treasury**; the May 2026 ARC whitepaper says stablecoin fees are "designed to be converted into ARC" under the future PoS economics (mechanism details pending) ([Arc token whitepaper blog](https://www.arc.io/blog/introducing-the-arc-token-whitepaper)).
- Observed testnet average cost: **~$0.009/tx** ([arc.io](https://www.arc.io/)).
- Circle **Paymaster** integration allows apps to sponsor/abstract gas for users ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).

## 4. Stablecoin, payments/FX, privacy, compliance

- **Native assets**: USDC, EURC, and USYC (Circle's tokenized money-market fund) are native to Arc; cirBTC (Circle-wrapped BTC) in testnet as of May 2026 ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance); [Crypto Times](https://www.cryptotimes.io/2026/05/22/circle-launches-cirbtc-testnet-ahead-of-wrapped-bitcoin-rollout/)).
- **FX engine ("StableFX")**: institutional-grade **RFQ price-discovery system with 24/7 PvP on-chain settlement** for stablecoin currency pairs (e.g., BRL, JPY, MXN, CAD stablecoins vs USDC/EURC), targeting replacement of prefunded accounts and T+1 FX settlement ([Decrypt](https://decrypt.co/348452/circle-unveils-on-chain-fx-engine-to-expand-stablecoin-trading-on-arc-network); [CoinGecko](https://www.coingecko.com/learn/what-is-arc-stablechain)).
- **Privacy**: **opt-in confidential transfers** — amounts shielded, addresses visible — implemented via a **Trusted Execution Environment (TEE)** producing attested results; plus "**Arc Privacy**," a confidential smart-contract engine preserving compliance/audit access ([crypto.news](https://crypto.news/circle-unveils-arc-privacy-to-bring-confidential-smart-contracts-to-institutions/); [Coin Bureau](https://coinbureau.com/education/what-is-arc-circle-stablechain)).
- **Compliance**: selective disclosure so enterprises can meet their own regulatory obligations; the chain also explores **"reversible transaction" / dispute-protocol concepts** (Circle President Heath Tarbert floated reversibility while maintaining settlement finality) — controversial and not confirmed as a shipped feature ([Coinpedia](https://coinpedia.org/news/circle-explores-reversible-transactions-sparking-debate-over-blockchains-core-principles/); [yellow.com research](https://yellow.com/research/circle-arc-blockchain-explained-how-reversible-transactions-work-and-what-they-mean-for-crypto)).
- **Tokenized assets / capital markets**: positioned for tokenized funds and collateral (BlackRock, Apollo, State Street, BNY, NYSE/ICE engagement) ([Circle pressroom](https://www.circle.com/pressroom/circle-launches-arc-public-testnet)).

## 5. Validator model / decentralization

- **Launch model: permissioned Proof-of-Authority.** Validators are known institutions selected for operational resilience, availability targets, security practices, geographic/jurisdictional diversity, and regulatory compliance ([Sentora](https://medium.com/sentora/some-technical-notes-about-circles-new-blockchain-d09b8d26e0a4); [CoinGecko](https://www.coingecko.com/learn/what-is-arc-stablechain)).
- **Roadmap: permissioned Proof-of-Stake.** The May 2026 ARC whitepaper frames the ARC token as enabling the PoA→PoS transition: validators operate the network, ARC holders stake, rewards come from inflation-funded issuance plus protocol fees; governance over fees/inflation/burn via staking ([Arc token whitepaper blog](https://www.arc.io/blog/introducing-the-arc-token-whitepaper); [Unchained](https://unchainedcrypto.com/circle-ceo-reveals-arc-network-token-plans-and-proof-of-stake-roadmap/); [Decrypt](https://decrypt.co/364295/circle-exploring-arc-network-token-proof-stake-shift-ceo)).
- **ARC tokenomics**: initial supply **10B**; **60% ecosystem/network participants, 25% Circle, 15% long-term reserve**. Presale placed **740M tokens at $0.30** (~7.4% of supply) ([Phemex](https://phemex.com/news/article/circle-unveils-arc-blockchain-whitepaper-mainnet-launch-set-for-summer-2026-82817); [The Block](https://www.theblock.co/post/400709/circle-raises-222m-in-arc-token-presale-at-3b-fdv-from-a16z-crypto-blackrock-and-others-q1-revenue-up-20)). Note: Circle initially messaged Arc as needing no native token (USDC gas); the token's introduction in 2026 was a notable evolution ([Decrypt](https://decrypt.co/364295/circle-exploring-arc-network-token-proof-stake-shift-ceo)).
- Circle's own risk disclosures flag **governance and conflict-of-interest risks** from its ARC holdings and control of Arc ([TipRanks](https://www.tipranks.com/news/company-announcements/circle-internet-group-faces-governance-conflict-of-interest-risks-from-arc-token-holdings-and-arc-control)).

## 6. Ecosystem and partners

100+ organizations engaged at testnet launch ([Circle pressroom](https://www.circle.com/pressroom/circle-launches-arc-public-testnet); [CryptoPotato](https://cryptopotato.com/circles-arc-blockchain-testnet-goes-live-over-100-partners-including-mastercard-coinbase-on-board/)):

- **Banks/asset managers**: BlackRock, Goldman Sachs, HSBC, Deutsche Bank, Standard Chartered, Société Générale, Commerzbank, BTG Pactual, Emirates NBD, First Abu Dhabi Bank, FirstRand, Absa, ClearBank, Bank Frick, Invesco, WisdomTree, SBI Holdings, Kyobo Life.
- **Payments/fintech/tech**: Visa, Mastercard, AWS, Cloudflare, Fiserv, FIS, Corpay, Nuvei, dLocal, EBANX, Paysafe, Brex, Ramp, Careem, Yellow Card, LianLian, Pairpoint by Vodafone, Sumitomo.
- **Capital markets**: Apollo, BNY Mellon, NYSE, State Street; **digital asset**: Coinbase, Kraken.
- **Presale investors** (May 2026): a16z crypto (lead, $75M), BlackRock, Apollo Funds, ICE, ARK Invest, Bullish, Haun Ventures, SBI, Janus Henderson, Standard Chartered Ventures, General Catalyst, Marshall Wace, IDG Capital ([The Block](https://www.theblock.co/post/400709/circle-raises-222m-in-arc-token-presale-at-3b-fdv-from-a16z-crypto-blackrock-and-others-q1-revenue-up-20)).
- **Circle-stack integration**: native USDC/EURC/USYC, Circle Mint (fiat on/off-ramp), Wallets, Contracts, **CCTP**, **Gateway**, **Paymaster**, Circle Payments Network (CPN) ([Circle blog](https://www.circle.com/blog/introducing-arc-an-open-layer-1-blockchain-purpose-built-for-stablecoin-finance)).
- **Developer traction**: 155 teams built on Arc at ETHGlobal HackMoney 2026 ([arc.network blog](https://www.arc.network/blog/meet-the-arc-track-winners-from-the-hackmoney-2026-hackathon-and-what-we-learned)); standard EVM tooling plus thirdweb, testnet explorer, faucets ([thirdweb](https://thirdweb.com/arc-testnet)).

## 7. Positioning vs other chains; criticisms

**Circle's framing**: Arc is the "Economic OS" for the internet financial system — a neutral-but-compliant settlement layer where USDC is money and gas, versus general-purpose chains (Ethereum: decentralized but slow/volatile fees; Solana: fast but volatile-fee consumer chain). Circle continues to support USDC on ~20+ chains; Arc is additive "home base" infrastructure, not a migration ([Blockworks](https://blockworks.com/news/circle-l1-impact-ethereum); [Circle 2026 product vision](https://www.circle.com/blog/building-the-internet-financial-system-circles-product-vision-for-2026)).

**Vs Stripe's Tempo**: Arc = USDC-native "money network" with dollar-denominated gas; Tempo = multi-stablecoin "commerce network" (any stablecoin as gas, ~100k TPS claims) — both start as permissioned/consortium-style chains ([Fintech Wrapup deep dive](https://www.fintechwrapup.com/p/deep-dive-stripe-and-circle-are-launching); [Across blog on stablechains](https://across.to/blog/stablechains); [kkdemian comparison](https://www.kkdemian.com/blog/arc_tempo_stablecoin)).

**Notable criticisms**:

- **Centralization**: Adam Cochran (Cinneamhain Ventures): "This isn't an L1 and it's offensive to call it such. It's a consortium chain of private pre-approved validators, who even have permission to refund transactions via 'dispute protocols'" ([The Defiant](https://thedefiant.io/news/blockchains/circle-s-arc-layer-1-re-ignites-the-open-versus-permissioned-chain-debate)).
- **Reversibility debate**: exploring reversible USDC transactions drew charges of being "anti-crypto" ([Coinpedia](https://coinpedia.org/news/circle-explores-reversible-transactions-sparking-debate-over-blockchains-core-principles/)).
- **"Walled garden for Wall Street"** critiques of permissioned validators + compliance-first design ([23studio](https://23stud.io/blog/circle-arc-blockchain-walled-garden-for-wall-street)).
- **$5,000 bug-bounty cap** for critical vulnerabilities drew security-community backlash at testnet launch ([Bitget News](https://www.bitget.com/news/detail/12560605357198)).
- **Token reversal** (initially no-native-token messaging, then a $222M presale) and Circle's conflicts of interest as issuer, validator gatekeeper, and 25% token holder ([TipRanks](https://www.tipranks.com/news/company-announcements/circle-internet-group-faces-governance-conflict-of-interest-risks-from-arc-token-holdings-and-arc-control)).

## 8. Interop / cross-chain settlement

- **CCTP (v2)** is a **native interoperability primitive** on Arc: canonical burn-and-mint USDC transfer across supported chains, with cross-chain message passing ([Circle blog: CCTP + Gateway on Arc](https://www.circle.com/blog/consolidate-crosschain-usdc-fast-low-cost-transfers-with-cctp-and-gateway)).
- **Circle Gateway** provides **chain-abstracted, unified USDC balances** with built-in liquidity rebalancing, consolidating fragmented cross-chain balances; combined with Arc's sub-second finality, Circle pitches Arc as a **cross-chain settlement hub** where liquidity routes into one high-speed environment ([same source](https://www.circle.com/blog/consolidate-crosschain-usdc-fast-low-cost-transfers-with-cctp-and-gateway)).
- **Circle Mint** handles fiat→USDC issuance directly on Arc; CPN (136 enrolled financial institutions, $8.3B annualized volume as of Q1 2026) is expected to settle over Arc ([Investing.com Q1 2026 coverage](https://www.investing.com/news/company-news/circle-q1-2026-slides-profit-surge-masks-revenue-miss-arc-launch-looms-93CH-4677001); [Circle Internet Financial System report](https://www.circle.com/reports/internet-financial-system/arc-and-circle-infrastructure)).
- Contract addresses for CCTP/Gateway on Arc testnet are published in the [Arc docs](https://docs.arc.io/arc/references/contract-addresses).

---

**Bottom line (mid-2026)**: Arc is in late-stage public testnet (~8 months live, ~250M+ cumulative transactions), with a summer 2026 mainnet-beta target, a newly announced ARC token ($222M presale, $3B FDV) to fund a PoA→PoS transition, USDC-gas/EWMA fee mechanics, Malachite BFT consensus (~350 ms finality, ~3k TPS demonstrated), deep Circle-stack integration (CCTP v2, Gateway, Mint, Paymaster, StableFX, Arc Privacy), and an unusually institutional partner roster — offset by persistent criticism over its permissioned validator set, reversibility explorations, and Circle's concentrated control.
