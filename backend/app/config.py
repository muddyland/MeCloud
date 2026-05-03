from functools import lru_cache
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

    @property
    def jmap_session_url(self) -> str:
        return f"{self.stalwart_url}/.well-known/jmap"

    @property
    def openid_config_url(self) -> str:
        return f"{self.stalwart_url}/.well-known/oauth-authorization-server"


@lru_cache
def get_settings() -> Settings:
    return Settings()
