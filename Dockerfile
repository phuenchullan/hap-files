<<<<<<< HEAD
FROM rust:1-bookworm AS builder
=======
FROM rust:1.85-bookworm AS build
>>>>>>> a81643640366fc33baf0e0355b50e26e29486cac
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

<<<<<<< HEAD
FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/hap-files /usr/local/bin/hap-files
=======
FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libssl3 \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --create-home --uid 10001 hap
COPY --from=build /app/target/release/hap-files /usr/local/bin/hap-files
RUN mkdir -p /data/images && chown -R hap:hap /data
USER hap
ENV BIND_ADDRESS=0.0.0.0:3001
ENV IMAGES_DIR=/data/images
EXPOSE 3001
>>>>>>> a81643640366fc33baf0e0355b50e26e29486cac
CMD ["hap-files"]
