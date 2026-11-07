FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p outhouse-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/outhouse-server /app/outhouse-server
COPY web /app/web
ENV WEB_DIR=/app/web
EXPOSE 8080
CMD ["/app/outhouse-server"]
