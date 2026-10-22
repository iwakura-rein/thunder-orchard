# The Rust version is NOT pinned by this tag — it comes from
# rust-toolchain.toml, which rustup in this image reads. The tag only
# bootstraps rustup, so it deliberately floats on the latest 1.x.
FROM rust:1-slim-bookworm AS builder
WORKDIR /workspace

# Install the pinned toolchain before copying the source, so the download
# is cached in its own layer and is not invalidated by code changes.
COPY rust-toolchain.toml .
RUN rustup toolchain install

COPY . .

RUN cargo build --locked --release

# Runtime stage
FROM debian:bookworm-slim

# Run as a non-root user so a compromised container cannot write host-mapped
# paths as root, and so wallet / cookie files created at runtime are not owned
# by uid 0.
RUN groupadd --system --gid 1000 thunder_orchard \
    && useradd --system --uid 1000 --gid thunder_orchard --home-dir /var/lib/thunder_orchard \
       --shell /usr/sbin/nologin thunder_orchard \
    && mkdir -p /var/lib/thunder_orchard \
    && chown -R thunder_orchard:thunder_orchard /var/lib/thunder_orchard

COPY --from=builder /workspace/target/release/thunder_orchard_app /bin/thunder_orchard_app
COPY --from=builder /workspace/target/release/thunder_orchard_app_cli /bin/thunder_orchard_app_cli

# Verify we placed the binary in the right place,
# and that it's executable.
RUN thunder_orchard_app --help

USER thunder_orchard
WORKDIR /var/lib/thunder_orchard
ENV HOME=/var/lib/thunder_orchard

ENTRYPOINT ["thunder_orchard_app"]
