# Optional registry prefix for the base images, e.g. a GitLab Dependency Proxy
# ("<host>/<group>/dependency_proxy/containers/") so CI pulls through a cache and
# avoids Docker Hub rate limits. Must include a trailing slash. Empty by default,
# so local builds pull straight from Docker Hub.
#
#   docker build --build-arg BASE_REGISTRY="${CI_DEPENDENCY_PROXY_GROUP_IMAGE_PREFIX}/" .
#
# Declared before the first FROM so every stage below can interpolate it.
ARG BASE_REGISTRY=

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
FROM ${BASE_REGISTRY}rust:1-trixie AS desktop-builder

ARG WITH_DESKTOP=1

RUN if [ "$WITH_DESKTOP" = "1" ]; then \
      apt-get update && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
        librsvg2-dev libsoup-3.0-dev libssl-dev pkg-config build-essential \
        zip ca-certificates \
      && rm -rf /var/lib/apt/lists/*; \
    fi

WORKDIR /build
COPY desktop/ ./desktop/

RUN mkdir -p /out && \
    if [ "$WITH_DESKTOP" = "1" ]; then \
      cd desktop/src-tauri && cargo build --release --locked || cargo build --release; \
      cd /build && sh desktop/package-linux.sh /out; \
    else \
      echo "WITH_DESKTOP=0 — skipping the desktop client"; \
    fi


# ── Stage 3: Install Python dependencies ─────────────────────────────────────
FROM ${BASE_REGISTRY}python:3.13-slim AS python-deps

WORKDIR /deps

COPY backend/requirements.txt ./
RUN pip install --no-cache-dir --prefix=/deps/install -r requirements.txt


# ── Stage 4: Final image ──────────────────────────────────────────────────────
FROM ${BASE_REGISTRY}python:3.13-slim

RUN useradd -m -u 1000 appuser

WORKDIR /app

# Copy Python packages from deps stage
COPY --from=python-deps /deps/install /usr/local

# Copy backend application code
COPY backend/ ./

# Copy built frontend static files
COPY --from=frontend-builder /app/build ./static

# The desktop client, served over a signed short-lived link (app/downloads.py).
# An empty directory is a valid outcome — see WITH_DESKTOP above.
COPY --from=desktop-builder /out/ ./downloads/

RUN chown -R appuser:appuser /app

USER appuser

ENV ENVIRONMENT=production \
    FRONTEND_STATIC_DIR=/app/static \
    DESKTOP_DOWNLOAD_DIR=/app/downloads \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8000/health')" || exit 1

# --proxy-headers so the app sees the real client IP behind a reverse proxy
# (per-IP rate limiting depends on it); --no-server-header so we don't advertise
# the exact server stack to scanners.
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", \
     "--proxy-headers", "--forwarded-allow-ips", "*", "--no-server-header"]
