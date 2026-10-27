FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY crates ./crates
COPY vendor ./vendor
# Bound temporary compiler/download storage without retaining it in build layers.
# Only the optimized executable survives this step. Requires BuildKit tmpfs mounts.
RUN --mount=type=tmpfs,target=/app/target,size=2147483648 \
    --mount=type=tmpfs,target=/usr/local/cargo/registry,size=1073741824 \
    --mount=type=tmpfs,target=/tmp,size=536870912 \
    df -B1 -T /app/target /usr/local/cargo/registry /tmp \
    && /usr/bin/prlimit --as=3221225472 -- /bin/sh -ec \
       'cat /proc/self/limits; exec cargo build --release --locked --jobs 2' \
    && install -m 0755 /app/target/release/stateset-freight /app/stateset-freight

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates poppler-utils util-linux fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/stateset-freight /usr/local/bin/stateset-freight
USER 10001:10001
ENV FREIGHT_BIND_ADDR=0.0.0.0:8090
EXPOSE 8090
HEALTHCHECK --interval=10s --timeout=3s --start-period=60s --retries=3 CMD ["/usr/local/bin/stateset-freight", "--check-ready"]
ENTRYPOINT ["stateset-freight"]
