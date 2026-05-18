# Zmieniamy tag na wersję opartą o debian-bookworm, żeby glibc się zgadzał
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# Zwróć uwagę na małą/wielką literę w nazwie binarnej (w builderze miałeś Slavia-backend)
RUN cargo build --release --bin Slavia-backend

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
    WORKDIR /app

    # Upewnij się, że nazwa pliku źródłowego (Slavia-backend) dokładnie odpowiada temu, co wypluwa cargo
    COPY --from=builder /app/target/release/Slavia-backend /usr/local/bin/slavia-backend
    EXPOSE 8080
    ENV PORT=8080
    CMD ["slavia-backend"]
    
