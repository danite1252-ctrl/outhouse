FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p outhouse-server
# deploy/push.ps1 writes VERSION (date + git commit); shown in Server status.
RUN (cat VERSION 2>/dev/null || true) > /version.txt

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/outhouse-server /app/outhouse-server
COPY --from=build /version.txt /app/version.txt
COPY web /app/web
ENV WEB_DIR=/app/web
EXPOSE 8080
CMD ["/app/outhouse-server"]
