# SN100 public site — Paper file audit (2026-09-08)

Marker: `SN100_SITE_PAPER_READY`

Non-normative design audit. It records what was drawn in the Paper file for
the Cortex Subnet 100 public website, which board holds what, and which
product facts every board was checked against. It is not a spec; when it
disagrees with [`../PROOF.md`](../PROOF.md), [`../BOUNTY.md`](../BOUNTY.md),
[`../SITE_API.md`](../SITE_API.md) or the miner guides under
[`../external-miner/`](../external-miner/), those documents win.

| Field | Value |
|-------|-------|
| Paper file | `01M1W80AKTEMHD0RTNA38GCZFN` — “Cortex Subnet 100” |
| Reference craft | `01M1RBMA2GZAQGQ3TZ4W73G4T6` — “Cortex Landing vNEW” (tokens mirrored, nothing copied) |
| Pages kept | `01 · Website` (`2-0`) · `02 · Research console` (`4-0`) · `03 · Design system` (`6-0`) · `04 · Responsive` (`5-0`) · `05 · Motion & states` (`3-0`) |
| Sources read before drawing | `README.md`, `docs/PROOF.md`, `docs/BOUNTY.md`, `docs/SITE_API.md`, `docs/external-miner/README.md`, `docs/external-miner/proof.md`, `docs/external-miner/bounty.md` |

## Canon applied (Subnet Owner correction)

- Routes drawn: `/` Home, `/proof`, `/bounty`, `/mine`, `/docs`, `/status`.
- Live on boards: Proof + Bounty only; control-plane shares Proof 80% /
  Bounty 20% (8000 / 2000 bps, sum 10000); public gateway
  `https://network.cortex.foundation`; eval image digest-pinned
  (`ghcr.io/cortexlm/proof-eval@sha256:78b614a1…`); RLM judge via
  `InferenceOffer` labelled **judge only, never a miner product**; eval GPU is
  **BYOK** (miner brings the Lium pod; the master does not rent 8×).
- Interim truth: an **“Emission · interim burn UID0”** callout sits on Home
  (hero ledger) and on Status (weights panel). No board promises live miner
  payout, an APY, a $ figure, a TAO price or a mainnet scoring date.
- Draft / not live, labelled as such on Home (“What exists today”), Mine,
  Status and the console board: live weights paying Proof miners, the prod
  RLM + GPU path, automatic reward-leaf emission, the whitepaper §7 synthesis
  agent.
- Off products: `relearn*`, `design`, `prism` appear only as “retired · no
  trust-root row · 404”. `coding` appears once as `paused` (site API frame).
  `chain.joinbase.ai` is not drawn anywhere.
- No invented topics, leaderboard names or hotkeys. The only topic document
  body shown is the documented operator example `dt-no-ib-v0`, framed
  “EXAMPLE · DRAFT · NOT LIVE”. Leaderboards and activity are drawn empty.

## Sampled live facts applied (2026-09-08)

A second pass folded in the research brief's sampled gateway state. Every
sampled value is labelled “sampled 2026-09-08” and “subject to change” on the
board; none of it weakens the burn-UID0 / no-payout-promise rule.

| Fact | Where it landed |
|------|-----------------|
| Proof `can_score: true`, open ids `dt-no-ib-v0`, `muon-vs-adamw-10m-v0` | Proof host-status panel (`can_score true`, `open_topics 2 · sampled`); topics table shows the two ids + `open`, with family / payout / budget deferred to each signed document (`GET /v1/proof/topics/{id}`); note that `can_score` is a readiness flag, not proof of reproduction or payment. `baseline_sealed` / `live_harvest_wired` are shown as “read live” (not sampled). Mirrored on the Proof dark twin, Proof 768 / 390, Home rows, Status health table and the console board. |
| Bounty `can_score: false`, scoring backend unconfigured | Bounty status panel (`scoring_backend unconfigured`, `can_score false`), note that `POST /v1/reports` answers 503 and the 2000 bps burn to uid 0; Home Bounty row; Status health table; console board. |
| `docs/BOUNTY.md` 7000 is doc drift | UI uses 2000 / 8000 only (unchanged). |
| Single digest `sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009` | Full digest on the Proof judge section; prefix elsewhere. No other digest appears. |
| Install CTA | Every terminal now carries the exact one-liner `curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh \| sh` (soft-wrapped visually where the column is narrower than the string, never split with inserted characters). |
| Stale BASE homepage (Design / Prism nav) | Not copied; nav is Proof · Bounty · Mine · Docs · Status. |
| Validators verify sealed weights, no evals; `chain.joinbase.ai` not branded | Mine board validators block; joinbase does not appear on any board. |

The empty “no open topic” table survives only as the Proof 503 failure state
on the states matrix and in the design-system state panels.

## Tokens rewritten in the file

Old black / Inter / Space Grotesk / Silkscreen set replaced. Names mirror
Landing vNEW so both files share a vocabulary.

| Token | Value | Use |
|-------|-------|-----|
| `--color-bg` / `--color-cream` | `#FAF8F4` | page ground, never pure white |
| `--color-text` / `--color-ink` / `--color-accent` | `#211F1C` | titles, body, CTA border + text |
| `--color-green` | `#1F4945` | footer band, eyebrows, share bar — never a CTA fill |
| `--color-green-hover` / `--color-green-tint-16` / `--color-accent-dim` | `#183936` / `#A9CCC7` / `#E8EAE6` | hover, bar secondary + focus ring, row hover |
| `--color-surface` / `--color-raised` | `#FFFFFF` / `#F3F0EA` | CTA fill, cards / code blocks, callouts |
| `--color-border` / `--color-hairline-strong` | `#E5E1D8` / `#D6D1C6` | rules |
| `--color-muted` / `--color-text-subtle` | `#6E6A62` / `#8B867C` | leads, meta |
| `--color-warning` / `--color-danger` | `#8A6A1F` / `#96412F` | interim / paused; 503, reject, penalty |
| `--color-bg-glass` / `--color-bg-glass-dark` | `#FAF8F4EB` / `#211F1CEB` | translucent nav over content |
| dark set | `--color-bg-dark #211F1C`, `--color-surface-dark #2A2724`, `--color-raised-dark #33302B`, `--color-hairline-dark #3A362F`, `--color-text-dark #EDEAE3`, `--color-text-muted-dark #9B968C`, `--color-text-subtle-dark #7D786E`, `--color-green-lift #3F958C` | dark twins |
| fonts | `--font-display` Newsreader (400) · `--font-body` Instrument Sans · `--font-mono` JetBrains Mono | display ≠ body |
| type scale | 64/68 hero · 56/60 h1 · 40/46 section · 32/40 heading · 24/30 title · 18/28 lead · 16/24 body · 14/20 label · 13/16 eyebrow (+0.08em) | |
| radii | `--radius-control 6px` · `--radius-card 12px` · `--radius-pill 999px` | |

Deleted: `--color-landing-*`, `--font-pixel`, `--font-landing`, `--radius-none`.

## Boards

All heights are measured fixed pixel heights (no `fit-content` stubs).

### `01 · Website` (page `2-0`)

| Board | Node | Size | Contents |
|-------|------|------|----------|
| 01 · Home · 1440 | `2UF-0` | 1440 × 4013 | glass nav; ruled-paper hero; emission ledger 80/20 + **interim burn UID0** callout; two live challenges rows (sampled state noted); research loop; how to mine (exact `ctx` install one-liner, BYOK note); live vs draft ledger; green footer |
| 02 · Proof · 1440 | `31A-0` | 1440 × 5878 | header + read-only host status (`can_score true · sampled 2026-09-08`, digest pinned, `open_topics 2`, harvest / baseline “read live”); topics table with the two sampled ids labelled operator-published · subject to change; topic anatomy + example document (draft, not live); submit flow + required POST JSON; digest-pinned RLM judge, verdict fields, cheat codes; `wta` vs `discovery`; 400 vs 503 table; BYOK callout |
| 03 · Bounty · 1440 | `3B9-0` | 1440 × 3601 | header + status panel (`scoring_backend unconfigured`, `can_score false` · sampled, quotas); pair → report → adjudicate; blocking terms; outcomes table (valid+severity, unpriced valid, already_fixed, malicious, duplicate); precision × impact with triage noise off the visible score; public leaderboard **empty, informational**, fail-closed scorer note |
| 04 · Mine · 1440 | `3HL-0` | 1440 × 2837 | install terminal; `ctx challenges` table (proof, bounty, coding paused, retired); bring-your-own-pod (you / the master / four checks); keys; validators fetch sealed weights only |
| 05 · Docs · 1440 | `3M2-0` | 1440 × 2364 | whitepaper card (§7 synthesis is a goal); read-in-order list linking out to repo paths; operator / architecture / contracts lists; frozen specs are gates only |
| 06 · Status · 1440 | `3M3-0` | 1440 × 2569 | weights panel `sealed: false · burn uid 0 = 100%` + **interim burn UID0** callout; emission shares 80/20 as configuration; five gateway endpoints with sampled states (Proof `can_score true · 2 open`, Bounty `can_score false · unconfigured`); network metagraph summary with `—` (0 when unknown, TAO price never shown); empty activity |
| 07 · Nav + Footer · components | `3VJ-0` | 1440 × 1693 | nav default, scrolled-over-table (glass), hover + focus, dark, 768, 390; footer band spec |
| 01 · Home · 1440 · Dark | `40F-0` | 1440 × 4013 | token-swapped twin |
| 02 · Proof · 1440 · Dark | `4BS-0` | 1440 × 5878 | token-swapped twin |
| 06 · Status · 1440 · Dark | `472-0` | 1440 × 2569 | token-swapped twin |

The four legacy boards on this page (old black system, wrong IA) were
deleted as authorised: `1-0`, `7Y-0`, `C2-0`, `CX-0`.

### `04 · Responsive` (page `5-0`)

| Board | Node | Size |
|-------|------|------|
| 01 · Home · 768 | `4LM-0` | 768 × 2464 |
| 01 · Home · 390 | `4LN-0` | 390 × 2155 |
| 02 · Proof · 768 | `4R6-0` | 768 × 1890 |
| 02 · Proof · 390 | `4R7-0` | 390 × 1948 |
| 03 · Bounty · 768 | `4R8-0` | 768 × 1962 |
| 03 · Bounty · 390 | `4R9-0` | 390 × 1915 |

### `03 · Design system` (page `6-0`)

| Board | Node | Size |
|-------|------|------|
| DS · Tokens, type, controls, tables, states | `51L-0` | 1440 × 2676 — light + dark swatches, type scale, buttons (default / hover / focus / disabled), inputs, chips, table pattern with hover + skeleton row, empty / loading / error / 403 / 503 / success panels, callouts |

### `05 · Motion & states` (page `3-0`)

| Board | Node | Size |
|-------|------|------|
| States matrix · Proof 503 · Bounty unpaired · Status chain down | `5A0-0` | 1440 × 1711 — each failure drawn in its page region with the exact copy and motion notes |

### `02 · Research console` (page `4-0`)

| Board | Node | Size |
|-------|------|------|
| Console · ctx status + verdict envelope (read-only) | `5D4-0` | 1440 × 904 — `ctx status` mirror with the sampled Proof / Bounty states and the submission verdict envelope shape, labelled “shape only · no row exists today” |

Legacy boards on the four support pages were kept, renamed with a `LEGACY`
prefix and moved to `y = 20000` so the new rows read first. They are the
rejected black system and can be deleted after review.

## Exports

`export` from the Paper MCP writes to the desktop client, not to this
repository. The seven 1440 Light boards were exported as `@2x` JPG to the
operator's machine:

```text
C:\Users\Mathis Work\Downloads\01 · Home · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\02 · Proof · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\03 · Bounty · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\04 · Mine · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\05 · Docs · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\06 · Status · 1440@2x.jpg
C:\Users\Mathis Work\Downloads\07 · Nav + Footer · components@2x.jpg
```

After the sampled-facts pass, Home, Proof, Bounty, Mine and Status were
re-exported to the same folder as `… @2x (1).jpg`; those five supersede the
first set. No raster is committed here; the Paper file is the artefact.

## Review notes

- The file rendered as a background tab in the desktop client during this
  session, so MCP screenshots came back black. Layout was verified from
  computed frame sizes (no clipped or overflowing text lane was left) and
  every board height was set from those measurements. A visual pass in the
  open file is still the acceptance step.
- Claude-style CTAs throughout: white fill, 1.5 px ink border, radius 6;
  hover inverts to ink fill with cream text. Green never fills a control.
- Numbers on boards are limited to documented values: 8000 / 2000 bps, the
  pinned digest prefix, quota limits (5 · 60 s · 80 · 20), pin floors
  (2e18, 0.02, 0.05), severity bounds (625 → 10000 bps), precision floor
  6000 bps, triage-noise ceiling 5000 bps.
