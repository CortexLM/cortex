FROM rust:1.75-slim as builder
WORKDIR /app
COPY . .
RUN apt-get update && apt-get install -y pkg-config libssl-dev && \
    cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ffmpeg libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/bounty-service /usr/local/bin/bounty-service
CMD ["bounty-service"]
