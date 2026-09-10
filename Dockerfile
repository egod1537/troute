FROM rust:1.98.1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bin troute

FROM debian:bookworm-slim AS runtime

COPY --from=builder /app/target/release/troute /usr/local/bin/troute
USER 10001:10001
ENV TROUTE_PORT=8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/troute"]
