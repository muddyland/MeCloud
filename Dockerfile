# ── Stage 1: Build SvelteKit frontend ────────────────────────────────────────
FROM node:20-alpine AS frontend-builder

WORKDIR /app

COPY frontend/package.json frontend/package-lock.json* ./
RUN npm install --prefer-offline

COPY frontend/ ./
RUN npm run build


# ── Stage 2: Install Python dependencies ─────────────────────────────────────
FROM python:3.12-slim AS python-deps

WORKDIR /deps

COPY backend/requirements.txt ./
RUN pip install --no-cache-dir --prefix=/deps/install -r requirements.txt


# ── Stage 3: Final image ──────────────────────────────────────────────────────
FROM python:3.12-slim

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

CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
