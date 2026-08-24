# External Miner: Bounty Submission Guide

## API Endpoint
`POST /v1/bounty/submit`

## Payload
Multipart form-data:
- `miner_id`: String (Your miner UID/ID)
- `video`: Binary (MP4/WebM bug reproduction video)

## Response
```json
{
  "id": "uuid-of-submission",
  "status": "PENDING"
}
```

## Rules
1. Videos must be original bug reproductions.
2. Submissions flagged as "SIMILAR" to an approved bug within the last 24 hours will be automatically rejected (HTTP 409 Conflict).
3. Approved bugs trigger the `score_epoch` emission cycle.
