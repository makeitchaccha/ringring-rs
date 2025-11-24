
FROM rust:1.91-slim-bookworm AS planner
LABEL authors="yuyaprgrm"
WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM rust:1.91-slim-bookworm AS builder
WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json

# install deps to build cosmic-text
RUN apt-get update && apt-get install -y \
    pkg-config \
    libfontconfig1-dev \
    libfreetype6-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin your-app-name

FROM debian:bookworm-slim AS runtime
WORKDIR /app

# install deps to run with cosmic-text
RUN apt-get update && apt-get install -y \
    libfontconfig1 \
    libfreetype6 \
    fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ringring-rs /app/ringring-rs
RUN useradd -m -u 1000 nonroot
USER nonroot:nonroot

ENTRYPOINT ["/app/ringring-rs"]