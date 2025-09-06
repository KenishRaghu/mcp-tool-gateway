# ---- build ----
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY gateway/Cargo.toml gateway/Cargo.lock ./
COPY gateway/src ./src
RUN cargo build --release --locked

# ---- runtime ----
# python3 is included so Harbor task verifiers/oracles can run in the same container.
FROM python:3.12-slim-bookworm
COPY --from=build /src/target/release/mcp-gateway /usr/local/bin/mcp-gateway
COPY gateway/gateway.toml /etc/mcp-gateway/gateway.toml
COPY gateway/policies /etc/mcp-gateway/policies
COPY --chmod=755 gateway/mcp-post.py /usr/local/bin/mcp-post
ENV GATEWAY_CONFIG=/etc/mcp-gateway/gateway.toml \
    GATEWAY_POLICY_DIR=/etc/mcp-gateway/policies \
    GATEWAY_WORKSPACE=/app/workspace \
    GATEWAY_AUDIT_LOG=/app/audit.jsonl \
    GATEWAY_LISTEN=0.0.0.0:8080 \
    GATEWAY_MODE=enforce
WORKDIR /app
EXPOSE 8080
CMD ["mcp-gateway"]
