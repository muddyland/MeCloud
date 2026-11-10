# Optional registry prefix for the base images, e.g. a mirror
# ("<host>/<path>/"). Must include a trailing slash. Empty by default, so
# builds pull straight from Docker Hub.
#
#   docker build --build-arg BASE_REGISTRY="mirror.example/library/" .
#
# Declared before the first FROM so every stage below can interpolate it.
ARG BASE_REGISTRY=

# The image the server ships on. `scratch` by default: the server is one
# statically linked binary with its CA roots compiled in, so there is nothing
# else to put in the image — no shell, no package manager, no libc. Any image
# can stand in, for instance a hardened static one; CI uses one when
# USE_DHI=true (Docker Hardened Images; see .gitlab-ci.yml).
#
#   docker build --build-arg RUNTIME_IMAGE=dhi.io/static:20261020-alpine3.24 .
ARG RUNTIME_IMAGE=scratch

# ── Stage 1: Build SvelteKit frontend ────────────────────────────────────────
FROM ${BASE_REGISTRY}node:24-alpine AS frontend-builder

WORKDIR /app

COPY frontend/package.json frontend/package-lock.json ./
# Strictly `npm ci`: it installs exactly the committed lockfile, and fails loudly
# if the lockfile has drifted from package.json. An `|| npm install` fallback
# looks forgiving but is worse — it turns "your lockfile is stale" into an
# ERESOLVE conflict deep in a half-resolved tree.
RUN npm ci --prefer-offline

COPY frontend/ ./
RUN node scripts/gen-icons.mjs && npm run build


# ── Stage 2: Build and package the Linux desktop client ─────────────────────
#
# Pinned to trixie deliberately: Tauri links against the system WebKitGTK, and
# a binary built against a newer one will not start on an older target. This is
# also why the client is Linux-only here — Windows and macOS binaries cannot be
# cross-compiled from this image and are built by the user from the source
# archive this stage also produces (see desktop/README.md).
#
# Skippable with --build-arg WITH_DESKTOP=0 when iterating on the web app: the
# Rust build is by far the slowest part of this Dockerfile. The image is still
# valid without it — /api/desktop/releases simply reports nothing to download.
#
# The compiler is pinned to a minor version, matching desktop/Dockerfile.build,
# so a rebuild of the same commit uses the same toolchain. Bump both together.
FROM ${BASE_REGISTRY}rust:1.99-trixie AS desktop-builder

ARG WITH_DESKTOP=1

RUN if [ "$WITH_DESKTOP" = "1" ]; then \
      apt-get update && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
        librsvg2-dev libsoup-3.0-dev libssl-dev pkg-config build-essential \
        zip ca-certificates \
      && rm -rf /var/lib/apt/lists/*; \
    fi

WORKDIR /build

# Dependencies first, in their own layer. Everything the build script and the
# dependency graph need, but not src/: with stub sources in place, editing the
# client's code (or its README) no longer recompiles ~540 crates.
COPY desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock desktop/src-tauri/build.rs \
     desktop/src-tauri/tauri.conf.json ./desktop/src-tauri/
COPY desktop/src-tauri/capabilities/ ./desktop/src-tauri/capabilities/
COPY desktop/src-tauri/icons/ ./desktop/src-tauri/icons/
COPY desktop/ui/ ./desktop/ui/
RUN if [ "$WITH_DESKTOP" = "1" ]; then \
      cd desktop/src-tauri && mkdir -p src && \
      echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs && \
      cargo build --release --locked && \
      rm -rf src target/release/mecloud-desktop* target/release/deps/mecloud_desktop* \
             target/release/.fingerprint/mecloud-desktop-*; \
    fi

COPY desktop/ ./desktop/

# --locked and nothing else. A `|| cargo build` fallback would quietly
# re-resolve a stale Cargo.lock — the very drift `npm ci` above refuses — and
# turn a real compile error into two full builds before failing.
RUN mkdir -p /out && \
    if [ "$WITH_DESKTOP" = "1" ]; then \
      cd desktop/src-tauri && cargo build --release --locked && \
      cd /build && sh desktop/package-linux.sh /out; \
    else \
      echo "WITH_DESKTOP=0 — skipping the desktop client"; \
    fi


# ── Stage 3: Build the server ────────────────────────────────────────────────
#
# Alpine, so the binary links musl statically and runs on `scratch`. The
# compiler is pinned to the same minor version as the desktop stage above.
FROM ${BASE_REGISTRY}rust:1.99-alpine AS server-builder

RUN apk add --no-cache musl-dev

# A crates.io mirror, as a cargo registry URL ("sparse+https://…/index/").
# Empty by default: crates come from crates.io.
ARG CARGO_REGISTRY_MIRROR=
RUN if [ -n "$CARGO_REGISTRY_MIRROR" ]; then \
      printf '[source.crates-io]\nreplace-with = "mirror"\n\n[source.mirror]\nregistry = "%s"\n' \
        "$CARGO_REGISTRY_MIRROR" > "$CARGO_HOME/config.toml"; \
    fi

WORKDIR /build/server

# Dependencies in their own layer, as for the desktop client: with stub
# sources in place, editing the server's code does not rebuild ~200 crates.
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs && \
    cargo build --release --locked && \
    rm -rf src target/release/mecloud-server target/release/deps/mecloud_server* \
           target/release/deps/libmecloud_server* target/release/.fingerprint/mecloud-server-*

COPY server/src/ ./src/
RUN cargo build --release --locked && \
    install -D -m 0755 target/release/mecloud-server /out/mecloud-server


# ── Stage 4: Final image ──────────────────────────────────────────────────────
#
# No RUN steps in this stage: `scratch` has nothing to run them with. The app
# is stateless and writes nothing to disk, so the files stay root-owned and
# read-only to the process, which runs as 65532 (the hardened images' own
# non-root user; nothing looks the user up, so no passwd entry is needed).
FROM ${RUNTIME_IMAGE}

WORKDIR /app

COPY --from=server-builder /out/mecloud-server /app/mecloud-server

# Copy built frontend static files
COPY --from=frontend-builder /app/build /app/static

# The desktop client, served over a signed short-lived link (server/src/downloads.rs).
# An empty directory is a valid outcome — see WITH_DESKTOP above.
COPY --from=desktop-builder /out/ /app/downloads/

USER 65532:65532

# Which peers may set X-Forwarded-For comes from FORWARDED_ALLOW_IPS. It
# defaults to loopback only: trusting "*" lets anyone who reaches the port
# directly pick their own IP, and with it their own rate-limit bucket. Set it to
# your reverse proxy's address (or its Docker network, e.g. 172.18.0.0/16) in
# .env.
ENV ENVIRONMENT=production \
    FRONTEND_STATIC_DIR=/app/static \
    DESKTOP_DOWNLOAD_DIR=/app/downloads \
    FORWARDED_ALLOW_IPS=127.0.0.1

EXPOSE 8000

# Exec form, and the server probing itself: there is no shell, curl or wget
# in the image to do it with.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/app/mecloud-server", "healthcheck"]

CMD ["/app/mecloud-server"]
