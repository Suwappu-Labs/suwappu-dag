# Excalidraw archive

Historical hand-drawn dark-mode canvases for the three core SUWAPPU layers
(DAG, DB, LTP). **These are retained for reference and live-editing
workflows only — Mermaid is the canonical visual source going forward.**

Each `.excalidraw` file opens in [Excalidraw](https://excalidraw.com)
(File → Open → select the file) and reproduces the corresponding
Mermaid diagram in a hand-drawn style. They were the earliest visual
format the repo carried; the inline-Mermaid
[`docs/visuals/README.md`](../README.md) and presentation HTML pages
now cover the same content with better cross-repo durability (Mermaid
renders natively on GitHub + GitBook; Excalidraw requires the web app).

## What lives here

- `suwappu-dag.excalidraw` — hand-drawn variant of the chain stack flow.
- `suwappu-db.excalidraw` — hand-drawn variant of the substrate lattice.
- `ltp.excalidraw` — hand-drawn variant of the transfer lifecycle.

## Why we don't sync these with Mermaid

Excalidraw's JSON format isn't a useful diff target — small visual
tweaks produce large position-coordinate changes that obscure the
semantic delta. Keeping these as a frozen historical reference is
cheaper than maintaining a sync pipeline. If a future contributor
wants live-editable diagrams, they can hand-edit a copy; the
canonical content lives in [`mermaid/`](../mermaid/).

## Related

- Canonical Mermaid sources: [`../mermaid/`](../mermaid/)
- Inline-rendered README: [`../README.md`](../README.md)
- HTML presentations: [`../index.html`](../index.html)
