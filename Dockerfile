FROM node:24-slim AS css
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY tailwind.config.js ./
COPY static/css/input.css static/css/input.css
COPY templates/ templates/
RUN npx tailwindcss -i static/css/input.css -o static/css/output.css --minify

FROM rust:1.94-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml server/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
RUN mkdir -p server/src cli/src && echo "fn main(){}" > server/src/main.rs && echo "fn main(){}" > cli/src/main.rs
RUN cargo build --release -p boopmark-server && rm -rf server/src cli/src
COPY server/ server/
COPY cli/ cli/
COPY migrations/ migrations/
COPY templates/ templates/
RUN touch server/src/main.rs && cargo build --release -p boopmark-server
RUN cargo build --release -p boopmark-server --example hash_password

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/boopmark-server .
COPY --from=builder /app/target/release/examples/hash_password .
COPY --from=builder /app/migrations/ migrations/
COPY --from=builder /app/templates/ templates/
COPY static/ static/
COPY --from=css /app/static/css/output.css static/css/output.css
EXPOSE 4000
CMD ["./boopmark-server"]
