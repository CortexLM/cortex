# External Miner Bounty Submission

## Endpoint
POST `/submit` on port `8095`

## Multipart Form Data
- `video`: The MP4 video file of the bug report.
- `miner_uid`: Your unique miner identifier.

## Edge Cases
- Bad multipart formatting will result in a `400 Bad Request`.
- If a similar video was submitted within the last 24h, you will receive a `409 Conflict`.
