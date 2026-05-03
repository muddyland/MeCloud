# Set env vars before any app import so get_settings() picks them up
import os
os.environ.update({
    "ENVIRONMENT":          "development",
    "STALWART_URL":         "http://mail.test",
    "OAUTH_CLIENT_ID":      "test-client",
    "OAUTH_CLIENT_SECRET":  "test-secret",
    "OAUTH_REDIRECT_URI":   "http://localhost:8000/auth/callback",
    "SESSION_SECRET":       "a" * 32,
    "APP_NAME":             "Test Mail",
    "FRONTEND_STATIC_DIR":  "/nonexistent",  # skip static-file mount
})

import pytest
from httpx import AsyncClient, ASGITransport

from app.config import get_settings
get_settings.cache_clear()

from app.main import app, require_auth


@pytest.fixture
async def client():
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as c:
        yield c


@pytest.fixture
async def auth_client():
    """Client with the auth dependency bypassed — returns a fake token."""
    app.dependency_overrides[require_auth] = lambda: "fake-access-token"
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as c:
        yield c
    app.dependency_overrides.pop(require_auth, None)
