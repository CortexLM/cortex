# Bounty Video Bug-Report Challenge

## Overview
This challenge enables miners to submit video bug-reports for bounty rewards.

## Workflow
1. Miner uploads video via multipart form to `/submit` on `:8095`.
2. Service compresses video using `ffmpeg`.
3. Service checks for duplicates using OpenRouter DeepSeek V4 Flash.
4. If duplicate found within 24h, returns `409 Conflict`.
5. Otherwise, saves as `PENDING` in PostgreSQL.
6. Admin approves via `/approve/:id` with `X-Admin-Token`.
7. Approved submissions trigger `score_epoch` TARGET=50 emission with uid0 burn sink.

## Trust Root Weights
Configured in `config/challenges.toml`:
- design: 3000 bps
- prism: 4500 bps
- bounty: 2500 bps

## Edge Cases
- Bad multipart: `400 Bad Request`
- Similarity reject within 24h: `409 Conflict`
- Unauthorized admin: `401 Unauthorized`
