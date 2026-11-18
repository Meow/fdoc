# syntax=docker/dockerfile:1

# The base images are pinned to a minor version: a rebuild picks up their
# patch releases, never a new minor.
FROM rust:1.100-alpine3.24 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY assets assets
COPY src src
RUN cargo build --release --locked --bin fdoc

FROM alpine:3.24
COPY --from=build /src/target/release/fdoc /usr/local/bin/fdoc
WORKDIR /src
ENTRYPOINT ["fdoc"]
