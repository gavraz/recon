FROM rust:1-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM ghcr.io/charmbracelet/vhs

RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux nodejs git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/recon /usr/local/bin/recon
COPY demo/demo.sh /demo/demo.sh
COPY demo/tapes /demo/tapes

RUN useradd -m -u 10001 recon \
    && mkdir -p /home/recon/.claude/sessions /home/recon/.claude/projects \
    && chown -R recon:recon /demo /home/recon
COPY --chown=recon:recon tmux.conf /home/recon/.tmux.conf

RUN chmod +x /usr/local/bin/recon /demo/demo.sh

WORKDIR /demo
USER recon
ENV HOME=/home/recon
