FROM rust:latest as builder

WORKDIR /usr/src/app
COPY src src
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
RUN cargo install --path .

FROM debian:bullseye-slim

COPY --from=builder /usr/local/cargo/bin/chamber /usr/local/bin/chamber

# HTTP
EXPOSE 3000

WORKDIR /root

CMD ["/usr/local/bin/chamber"]
