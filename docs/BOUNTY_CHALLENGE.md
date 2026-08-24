# Bounty Challenge

## Overview
This challenge implements a video-based bounty submission workflow. Miners can upload bug-report videos which are compressed and checked for similarity.

## Architecture
- Axum-based bounty service on `:8095`
- PostgreSQL persistence
- ffmpeg video compression
- OpenRouter DeepSeek V4 Flash similarity checks

## Configuration
Trust root weights are configured in `config/challenges.toml`:
- design: 3000 bps
- prism: 4500 bps
- bounty: 2500 bps

## Admin Approval
Administrators can approve submissions via the `/approve` endpoint using the `X-Admin-Token` header. Approved submissions trigger the `score_epoch` TARGET=50 emission with uid0 burn sink.
