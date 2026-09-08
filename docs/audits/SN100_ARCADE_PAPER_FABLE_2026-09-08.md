# SN100 Arcade — Paper design audit (Fable 5.1, 2026-09-08)

**Marker:** **SN100_ARCADE_PAPER_READY**

Design-only audit trail for the Cortex Subnet 100 marketing site drawn in Paper.
This file records board ids, tokens, the Overmind measurements the system is
based on, and the operator locks that were applied. It is not a product spec:
product truth stays in [`../OVERVIEW.md`](../OVERVIEW.md),
[`../external-miner/`](../external-miner/) and [`../SITE_API.md`](../SITE_API.md).
Nothing in the design changes emission, scoring or consensus semantics.

## Paper file

- File: **Cortex 100 Arcade** — `https://app.paper.design/file/01M204NFTFSVRW6QG78AC7EM6B`
- Pages: `1-0` Page 1 (cover) · `2-0` 01 Design System · `3-0` 02 Components ·
  `4-0` 03 Website 1440 · `5-0` 04 Responsive · `6-0` 05 Motion & SFX

### Board ids

| Page | Board | Node id |
|------|-------|---------|
| Page 1 | 00 · Cover + Read me · SN100_ARCADE_PAPER_READY | `39P-0` |
| 01 Design System | DS 01 · Tokens + Type · 1440 | `2AR-0` |
| 01 Design System | DS 02 · Controls, Tables, States, Usage Rules · 1440 | `2DL-0` |
| 02 Components | Nav · Floating · 1440 (54px) — nav shell `1LW-0` | `1-0` |
| 02 Components | Nav · Challenges dropdown open · 1440 | `1OJ-0` |
| 02 Components | Footer · 1440 (320px) — footer `2M-0` | `2L-0` |
| 02 Components | Arcade Kit · icon tiles `3AE-0` · process flow `3DR-0` · LLM alert `3FM-0` · agent button `3G6-0` · agent prompt card `3GJ-0` | `3AA-0` |
| 03 Website 1440 | 01 · Home · 1440 (hero `4D-0`, mining flow `6F-0`, weight flow `9Z-0`, three ways to start `CC-0`) | `4C-0` |
| 03 Website 1440 | 01b · Home · Start mining modal · 1440 | `3OO-0` |
| 03 Website 1440 | 02 · Proof · 1440 (hero `HY-0`, Mine prompt card `MP-0`) | `HX-0` |
| 03 Website 1440 | 03 · Bounty · 1440 (hero `RH-0`, flow + Mine prompt card `U3-0`) | `RG-0` |
| 03 Website 1440 | 04 · Mine · 1440 (hero `ZW-0`, two doors `13H-0`) | `ZV-0` |
| 03 Website 1440 | 05 · Docs · 1440 | `175-0` |
| 03 Website 1440 | 06 · Status · 1440 | `1CW-0` |
| 03 Website 1440 | BG · Scanline Tile 1440×600 (clone source `HA-0`) | `H9-0` |
| 04 Responsive | 01 · Home · 768 / 390 | `2HS-0` / `2ML-0` |
| 04 Responsive | 02 · Proof · 768 / 390 | `2QL-0` / `2QM-0` |
| 04 Responsive | 03 · Bounty · 768 / 390 | `2QN-0` / `2QO-0` |
| 05 Motion & SFX | Motion + SFX + Fail-closed states · 1440 | `36V-0` |

## Overmind steal notes (measured live, 2026-09-08)

Source: `https://www.overmindlab.ai/`, headless capture at 1440 and 390 plus the
six operator screenshots in `overmind-audit/`.

| Element | Measured | Cortex remap |
|---------|----------|--------------|
| Nav shell | floats 12px from top, 1312 wide (64px inset each side), 54px tall, 4px stepped corners, `#201c19` fill, `#3d3934` frame, no blur | identical geometry, notches drawn as 4px squares; `clip-path` polygon in code |
| Nav content | pixel-caption links left, logo orb centre, text link + filled copper CTA right | Challenges ▾ · Benchmarks · Docs · Status · orb · Whitepaper · **Start mining** (green `#1F4945`) |
| Page | `#16120f` page, 1px white scanlines at ~8% every 6px, vignette, pixel starfield | same values; scanlines as one SVG path tile cloned per board |
| Accent | copper `#ed670f` / `#fa680c` | structural green `#1F4945`, hover `#183936`, lift `#3F958C` (text/LEDs), CRT glow `#00ff80` at 10% only |
| Type | mondwest display, neueBit body, Pixelify Sans captions, Geist Pixel alt | Handjet 600 display, DotGothic16 body, Pixelify Sans 500 captions, Press Start 2P numerals, JetBrains Mono terminals, Inter for Docs prose only |
| Components | outlined section tags, dotted framed panels with corner ornaments, two-column ledger tables, cream `#f0ece1` GET STARTED CTA, product-UI screenshots | same language: tags, framed panels, ledgers, HUD strips, CRT bezel terminal; no icon-tile feature grids |
| SFX | fixed bottom-left SOUND ON toggle, pixel hand cursor on hover | kept; spec on the Motion board |

## Tokens (Paper design tokens, Tailwind v4 names)

Colour: `--color-canvas #16120F`, `--color-panel #201C19`, `--color-raised #2A2521`,
`--color-hairline #3D3934`, `--color-hairline-strong #57514A`, `--color-ink #F6F5F1`,
`--color-ink-muted #CECDC9`, `--color-ink-dim #8E8A83`, `--color-white #FFFFFF`,
`--color-brand #1F4945`, `--color-brand-hover #183936`, `--color-brand-deep #122E2B`,
`--color-brand-lift #3F958C`, `--color-brand-tint rgba(31,73,69,.24)`,
`--color-cream #F0ECE1`, `--color-cream-ink #16120F`, `--color-ok #3F958C`,
`--color-warn #E0A93A`, `--color-danger #E5484D`, `--color-crt-glow rgba(0,255,128,.10)`,
`--color-scanline rgba(255,255,255,.08)`.

Arcade accents (tiles, tile edges and LEDs only): `--color-arcade-green #3F958C`,
`--color-arcade-amber #E0A93A`, `--color-arcade-coral #E5484D`, `--color-arcade-cyan #4FB3D9`,
`--color-arcade-violet #8B7CF6`, `--color-arcade-pink #E86AA6`, `--color-arcade-lime #9BD35A`,
`--color-arcade-cream #F0ECE1`; modal backdrop `--color-backdrop rgba(11,9,8,.78)`.

Type: `--font-display Handjet`, `--font-body DotGothic16`, `--font-hud Pixelify Sans`,
`--font-score Press Start 2P`, `--font-mono JetBrains Mono`, `--font-prose Inter`;
sizes `--text-hud-xs 10` … `--text-display-2xl 88`; `--tracking-hud 0.06em`,
`--tracking-display 0`.

Layout: `--container-nav 1312`, `--container-content 1120`, `--container-prose 720`;
spacing 4/8/12/16/24/32/48/64/96/128; radii all `0` (pixel notches) except
`--radius-pill 999` for status pills; breakpoints 390 / 768 / 1440.

## Operator locks applied

1. **Navbar** — Challenges (game-menu dropdown with arcade icons: joystick, cabinet,
   bug, padlock for retired), Benchmarks, Docs, Status, centred orb, Whitepaper,
   primary CTA **Start mining**. No "Install CTX" button label anywhere; the install
   one-liner stays in body terminals.
2. **Hero art** — generated pixel art with a transparent background, one dedicated
   piece per route, next to the H1: Home cabinet, Proof trophy, Bounty bug in a
   crosshair, Mine pickaxe and coins. Palette `#1F4945` / `#F0ECE1` / `#F6F5F1`
   only. Assets: [`assets/`](./assets/) (`sn100-arcade-cabinet.png`,
   `sn100-arcade-proof-trophy.png`, `sn100-arcade-bounty-bug.png`,
   `sn100-arcade-mine-pickaxe.png`). Responsive boards mirror the art.
3. **Emission copy** — every board writes emission as **% of emissions**
   (Proof 80% of emissions, Bounty 20% of emissions). No raw protocol units or bare
   split integers as the primary metric on any UI board (Website, Components,
   Responsive, Design System usage rules).
4. **Humans first (product UX rewrite, 2026-09-08 pm)** — no terminal or code-block
   mockups, no endpoint paths, no status codes, no shell commands and no snake_case
   identifiers on any marketing board. Every former "API ladder" is now a plain-language
   card or a process flow: a coin travels a dashed track between coloured arcade
   stations, with the motion written out beside it (Home mining flow `6F-0`, Home
   six-station weight flow `9Z-0`, Bounty flow inside `U3-0`).
5. **Mine buttons + prompt cards** — Proof and Bounty heroes carry a primary **MINE**
   button; pressing it reveals a card with the human-readable mining prompt for that
   challenge (cream ticket, COPY PROMPT). Home **Start mining** opens a generalist
   modal (`3OO-0`): hand it to your agent / do it by hand in four steps / read first.
6. **LLM alert** — a strip under the nav on every 1440 page and a compact line on the
   responsive boards: "Point this to your LLM to mine or understand the project" →
   `https://network.cortex.foundation/llms.txt`.
7. **Coloured arcade tiles** — one accent per idea, tiles and LEDs only: green Proof,
   amber Bounty, cyan judge, violet topic, pink payout, lime your agent, coral
   fail-closed, cream hotkey. Tokens `--color-arcade-*`, `--color-backdrop`.

## Owner canon carried on the boards

- Gateway `https://network.cortex.foundation`; routes `/`, `/proof`, `/bounty`,
  `/mine`, `/docs`, `/status`.
- Live challenges: Proof 80% of emissions, Bounty 20% of emissions, nothing else.
- Interim burn to UID 0 shown on Home (HUD cell, honesty ledger, sample table) and
  Status (burn banner, emission ledger). No payout, APY or TAO price promises.
- InferenceOffer is the RLM judge only; miners BYOK Lium; one digest-pinned eval
  image (`sha256:78b614a1…`).
- `relearn*`, `design`, `prism` appear only as retired / locked / 404.
- Sampled 2026-09-08 (label subject to change): Proof `can_score true`, topics
  `dt-no-ib-v0`, `muon-vs-adamw-10m-v0`; Bounty `can_score false`,
  `scoring_backend unconfigured`.

## Agent prompts shown on the boards

The prompt cards carry three human-readable prompts (Home generalist, Proof, Bounty).
They tell the agent to read `https://network.cortex.foundation/llms.txt` first, check
that the challenge is scoring today, never handle a mnemonic, never invent results,
and stop when the network says it cannot score. They describe the miner's job in one
paragraph; they are not CLI transcripts.

## Exports (@2x PNG, written to the operator's Downloads by Paper)

Human-first rewrite set: `01 · Home · 1440@2x (2).png`, `01b · Home · Start mining modal · 1440@2x.png`,
`02 · Proof · 1440@2x (1).png`, `03 · Bounty · 1440@2x (1).png`, `04 · Mine · 1440@2x (1).png`,
`05 · Docs · 1440@2x (1).png`, `06 · Status · 1440@2x (1).png`,
`Arcade Kit · icon tiles · process flow · agent card · LLM alert · 1440@2x.png`,
`Proof · Mine prompt card + outcomes@2x.png`, `Bounty · flow + Mine prompt card@2x.png`,
`Home · mining flow + HUD@2x.png`, `Home · six-station weight flow@2x.png`,
`Home · three ways to start@2x.png`, `Mine · two doors + never@2x.png`,
`Motion + SFX + Fail-closed states · 1440@2x (1).png`.

Earlier set (still valid for nav and hero art):

`01 · Home · 1440@2x.png`, `02 · Proof · 1440@2x.png`, `03 · Bounty · 1440@2x.png`,
`04 · Mine · 1440@2x.png`, `05 · Docs · 1440@2x.png`, `06 · Status · 1440@2x.png`,
`Hero · Home · arcade cabinet@2x.png`, `Hero · Proof · arcade trophy@2x.png`,
`Hero · Bounty · arcade bug@2x.png`, `Hero · Mine · arcade pickaxe@2x.png`,
`Nav · Floating · 1440 (54px)@2x.png`, `Nav · Challenges dropdown open · 1440@2x.png`,
`DS 01 · Tokens + Type · 1440@2x.png`, `DS 02 · Controls, Tables, States, Usage Rules · 1440@2x.png`,
`01 · Home · 768@2x.png`, `01 · Home · 390@2x.png`, `02 · Proof · 768@2x.png`,
`02 · Proof · 390@2x.png`, `03 · Bounty · 768@2x.png`, `03 · Bounty · 390@2x.png`,
`Motion + SFX + Fail-closed states · 1440@2x.png`.

The file is one coherent dark CRT system; there is no separate light mode board.

## Known gaps

- Mondwest, neueBit and Geist Pixel are not available in Paper; Handjet and
  DotGothic16 stand in. Swap when licensed faces are provided.
- Paper does not render `clip-path`; notched corners are drawn with 4px squares.
- Hero PNGs are referenced by raw GitHub URL from this branch; repoint to the site
  asset pipeline when the site is built.
