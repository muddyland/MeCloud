from pydantic import BaseModel, Field


class JMAPRequest(BaseModel):
    using: list[str] = Field(max_length=20)
    methodCalls: list = Field(max_length=50)


class TokenResponse(BaseModel):
    access_token: str
    refresh_token: str | None = None
    token_type: str = "bearer"
    expires_in: int | None = None
    scope: str | None = None
