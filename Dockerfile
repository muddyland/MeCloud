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


# ── Stage 2: Install Python dependencies ─────────────────────────────────────
FROM ${BASE_REGISTRY}python:3.13-slim AS python-deps

WORKDIR /deps

COPY backend/requirements.txt ./
RUN pip install --no-cache-dir --prefix=/deps/install -r requirements.txt


# ── Stage 3: Final image ──────────────────────────────────────────────────────
FROM ${BASE_REGISTRY}python:3.13-slim

RUN useradd -m -u 1000 appuser

WORKDIR /app

# Copy Python packages from deps stage
COPY --from=python-deps /deps/install /usr/local

# Copy backend application code
COPY backend/ ./

# Copy built frontend static files
COPY --from=frontend-builder /app/build ./static

RUN chown -R appuser:appuser /app

USER appuser

ENV ENVIRONMENT=production \
    FRONTEND_STATIC_DIR=/app/static \
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
