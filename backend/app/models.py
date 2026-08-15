from typing import Any

from pydantic import BaseModel, Field, field_validator


class JMAPRequest(BaseModel):
    """A JMAP request envelope (RFC 8620 §3.3).

    The proxy forwards these verbatim with the session's bearer token attached,
    so the shape is validated here rather than trusting whatever the browser
    posted: a method call is exactly ``[name, arguments, callId]``.
    """

    using: list[str] = Field(min_length=1, max_length=20)
    methodCalls: list[Any] = Field(min_length=1, max_length=50)

    @field_validator("using")
    @classmethod
    def _check_capabilities(cls, value: list[str]) -> list[str]:
        for urn in value:
            if not urn or len(urn) > 255:
                raise ValueError("capability URNs must be 1-255 characters")
        return value

    @field_validator("methodCalls")
    @classmethod
    def _check_method_calls(cls, value: list[Any]) -> list[Any]:
        for call in value:
            if not isinstance(call, (list, tuple)) or len(call) != 3:
                raise ValueError("each method call must be [name, arguments, callId]")
            name, arguments, call_id = call
            if not isinstance(name, str) or not (0 < len(name) <= 128):
                raise ValueError("method name must be a string of 1-128 characters")
            if not isinstance(arguments, dict):
                raise ValueError("method arguments must be an object")
            if not isinstance(call_id, str) or not (0 < len(call_id) <= 64):
                raise ValueError("method callId must be a string of 1-64 characters")
        return value


class TokenResponse(BaseModel):
    access_token: str
    refresh_token: str | None = None
    token_type: str = "bearer"
    expires_in: int | None = None
    scope: str | None = None
