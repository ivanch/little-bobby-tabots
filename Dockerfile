# Dashboard Build Stage
FROM node:24-alpine AS web-builder

WORKDIR /usr/src/little-bobby-tabots/web

COPY web/package.json web/package-lock.json ./
RUN npm ci

COPY web ./
RUN npm run build

# Rust Build Stage
FROM rust:alpine AS builder

# Install build dependencies, including git, static OpenSSL, build-base, and cmake
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig git build-base cmake

WORKDIR /usr/src/little-bobby-tabots

# Force static linking of OpenSSL
ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/usr

# Copy dependency manifests and build a dummy main to cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Remove dummy build artifacts and copy the actual source code
RUN rm -f target/release/deps/little_bobby_tabots* src/main.rs
COPY src ./src

# Compile the final release binary
RUN cargo build --release

# Runtime Stage
FROM alpine:latest

# Install python3 and ffmpeg
RUN apk add --no-cache ffmpeg python3 curl

# Install uv and use it to install the latest yt-dlp securely
RUN curl -LsSf https://astral.sh/uv/install.sh | sh && \
    /root/.local/bin/uv tool install yt-dlp

ENV PATH="/root/.local/bin:${PATH}"

# Copy the compiled static binary from the builder stage
COPY --from=builder /usr/src/little-bobby-tabots/target/release/little-bobby-tabots /usr/local/bin/little-bobby-tabots
COPY --from=web-builder /usr/src/little-bobby-tabots/web/dist /opt/little-bobby-tabots/web

# Set runtime env defaults
ENV GUILD_ID=""
ENV RUST_LOG="info"
ENV WEB_BIND="0.0.0.0:3000"
ENV DASHBOARD_DIR="/opt/little-bobby-tabots/web"
ENV PLAYLISTS_DIR="/playlists"

EXPOSE 3000

CMD ["little-bobby-tabots"]
