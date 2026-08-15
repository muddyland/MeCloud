# ── Stage 1: Build SvelteKit frontend ────────────────────────────────────────
FROM node:24-alpine AS frontend-builder

WORKDIR /app

COPY frontend/package.json frontend/package-lock.json* ./
# `npm ci` when the lockfile is in sync with package.json, `npm install` otherwise
# (which also refreshes the lockfile inside the image).
RUN npm ci --prefer-offline || npm install --prefer-offline

COPY frontend/ ./
RUN node scripts/gen-icons.mjs && npm run build


# ── Stage 2: Install Python dependencies ─────────────────────────────────────
FROM python:3.13-slim AS python-deps

WORKDIR /deps

COPY backend/requirements.txt ./
RUN pip install --no-cache-dir --prefix=/deps/install -r requirements.txt


# ── Stage 3: Final image ──────────────────────────────────────────────────────
FROM python:3.13-slim

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
