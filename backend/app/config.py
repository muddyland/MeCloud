from functools import lru_cache
from urllib.parse import urlparse

from pydantic import model_validator
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8", extra="ignore")

    app_name: str = "JMAP Mail"
    stalwart_url: str = "https://mail.example.com"
    oauth_client_id: str
    oauth_client_secret: str
    oauth_redirect_uri: str = "http://localhost:8000/auth/callback"
    session_secret: str
    oauth_scope: str = "openid offline_access urn:ietf:params:jmap:core urn:ietf:params:jmap:mail"
    frontend_static_dir: str = "static"
    host: str = "0.0.0.0"
    port: int = 8000
    environment: str = "production"   # "production" | "development"
    app_url: str = ""                  # required in production (e.g. https://mail.example.com)

    # ── Hardening knobs ──────────────────────────────────────────────────────
    # Idle lifetime of the session cookie. The cookie is re-issued on every
    # request, so this is a sliding window, not an absolute cap.
    session_max_age: int = 60 * 60 * 24 * 7        # 7 days

    # Largest JSON body we will accept on the JMAP proxy. JMAP requests are
    # small; anything larger is either a bug or an attempt to exhaust memory.
    max_request_bytes: int = 1_048_576             # 1 MiB

    # File uploads get their own, much larger ceiling. Keeping these separate
    # matters: the 1 MiB cap is what stops a hostile JMAP payload, and raising
    # it everywhere just to allow file uploads would throw that away. Stalwart's
    # own FileStorage.maxSize defaults to 25 MB, so this only needs to be
    # generous enough not to be the binding constraint.
    max_upload_bytes: int = 64 * 1024 * 1024       # 64 MiB

    # Upstream (Stalwart) HTTP timeouts, in seconds. Without these a hung
    # upstream pins a worker and its connection forever.
    upstream_connect_timeout: float = 5.0
    upstream_read_timeout: float = 30.0

    # Per-user cap on concurrent Server-Sent Event streams. Each stream holds an
    # upstream connection open, so an unbounded count is a self-inflicted DoS.
    max_event_streams: int = 64

    @property
    def is_production(self) -> bool:
        return self.environment.lower() != "development"

    @model_validator(mode="after")
    def _check_app_url_in_production(self) -> "Settings":
        if self.is_production and not self.app_url:
            raise ValueError(
                "APP_URL must be set when ENVIRONMENT is 'production'. "
                "Set ENVIRONMENT=development for local development without HTTPS."
            )
        return self

    @model_validator(mode="after")
    def _check_session_secret_strength(self) -> "Settings":
        # The session secret is the only thing standing between a cookie and the
        # OAuth tokens sealed inside it — refuse to boot with a placeholder.
        if self.is_production:
            if len(self.session_secret) < 32:
                raise ValueError("SESSION_SECRET must be at least 32 characters in production.")
            if "change-me" in self.session_secret.lower():
                raise ValueError("SESSION_SECRET is still the example placeholder — generate a real one.")
        return self

    @property
    def jmap_session_url(self) -> str:
        return f"{self.stalwart_url}/.well-known/jmap"

    @property
    def openid_config_url(self) -> str:
        return f"{self.stalwart_url}/.well-known/oauth-authorization-server"

    @property
    def allowed_hosts(self) -> list[str]:
        """Host header allow-list. Everything else gets a 400 before routing."""
        if not self.is_production:
            return ["*"]
        hosts = {"localhost", "127.0.0.1"}      # container healthcheck
        for url in (self.app_url, self.oauth_redirect_uri):
            host = urlparse(url).hostname
            if host:
                hosts.add(host)
        return sorted(hosts)


@lru_cache
def get_settings() -> Settings:
    return Settings()
