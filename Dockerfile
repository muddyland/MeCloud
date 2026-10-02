# Optional registry prefix for the base images, e.g. a mirror
# ("<host>/<path>/"). Must include a trailing slash. Empty by default, so
# builds pull straight from Docker Hub.
#
#   docker build --build-arg BASE_REGISTRY="mirror.example/library/" .
#
# Declared before the first FROM so every stage below can interpolate it.
ARG BASE_REGISTRY=

# The Python images for the dependency stage and the final stage. Both default
# to python:3.14-slim. To build on a hardened, shell-less runtime instead, point
# them at a matching pair: a "-dev" image with a shell and pip to install into,
# and its runtime sibling to ship. CI does this when USE_DHI=true (Docker
# Hardened Images; see .gitlab-ci.yml).
#
#   docker build \
#     --build-arg PYTHON_BUILDER_IMAGE=dhi.io/python:3.14-alpine-dev \
#     --build-arg PYTHON_RUNTIME_IMAGE=dhi.io/python:3.14-alpine .
#
# The two must come from the same family: packages built against glibc in a
# slim builder will not load on a musl (Alpine) runtime, and vice versa.
ARG PYTHON_BUILDER_IMAGE=${BASE_REGISTRY}python:3.14-slim
ARG PYTHON_RUNTIME_IMAGE=${BASE_REGISTRY}python:3.14-slim

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
FROM ${PYTHON_BUILDER_IMAGE} AS python-deps

WORKDIR /deps

COPY backend/requirements.txt ./
# --target rather than --prefix or a venv: a plain directory on PYTHONPATH
# does not depend on where the interpreter lives, which differs between the
# slim image (/usr/local) and the hardened one (/usr).
RUN pip install --no-cache-dir --target=/opt/pydeps -r requirements.txt


# ── Stage 4: Final image ──────────────────────────────────────────────────────
#
# No RUN steps in this stage. The hardened runtime has no shell, so anything
# here must be a COPY or metadata. The app is stateless and writes nothing to
# disk, so the files stay root-owned and read-only to the process. It runs as
# 65532, the hardened image's own non-root user. Slim has no passwd entry for
# that uid, which is fine because nothing looks the user up.
FROM ${PYTHON_RUNTIME_IMAGE}

WORKDIR /app

COPY --from=python-deps /opt/pydeps /opt/pydeps

# Copy backend application code
COPY backend/ ./

# Copy built frontend static files
COPY --from=frontend-builder /app/build ./static

# The desktop client, served over a signed short-lived link (app/downloads.py).
# An empty directory is a valid outcome — see WITH_DESKTOP above.
COPY --from=desktop-builder /out/ ./downloads/

USER 65532:65532

ENV ENVIRONMENT=production \
    FRONTEND_STATIC_DIR=/app/static \
    DESKTOP_DOWNLOAD_DIR=/app/downloads \
    PYTHONPATH=/opt/pydeps \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

EXPOSE 8000

# Exec form: there is no shell to run a shell-form command on the hardened
# image. urlopen raises on a refused connection or a non-2xx, which exits 1.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["python", "-c", "import urllib.request; urllib.request.urlopen('http://localhost:8000/health')"]

# --proxy-headers so the app sees the real client IP behind a reverse proxy
# (per-IP rate limiting depends on it); --no-server-header so we don't advertise
# the exact server stack to scanners.
# `python -m`: the uvicorn launcher script is in /opt/pydeps/bin, not on PATH.
CMD ["python", "-m", "uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", \
     "--proxy-headers", "--forwarded-allow-ips", "*", "--no-server-header"]
