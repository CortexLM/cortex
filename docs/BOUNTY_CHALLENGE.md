# Bounty Video Bug-Report Challenge

## Overview
This challenge introduces a video-based bug reporting mechanism for miners. Submissions undergo automated compression via `ffmpeg`, similarity rejection using OpenRouter's DeepSeek V4 Flash (within a 24h window), and manual admin approval.

## Architecture
- **Miner Multipart Upload**: Submits video and metadata to `:8095/v1/bounty/submit`.
- **FFmpeg Compress**: Server-side compression to standardise storage and bandwidth.
- **OpenRouter DeepSeek V4 Flash**: 24h similarity check to prevent duplicate spam.
- **Admin Approve**: Manual verification before emission.
- **Score Epoch**: Emits `TARGET=50` with `uid0` burn sink. Trust root weights are distributed as: Design (3000 bps), Prism (4500 bps), Bounty (2500 bps).

## Deployment
Service is wired via Docker Compose on port `:8095`. Ensure `ffmpeg` is installed in the container image.
