"""Authenticated Casdoor identity shared by the sync API and adapter."""
from dataclasses import dataclass


class IdentityUnavailable(Exception):
    pass


@dataclass(frozen=True)
class Identity:
    issuer: str
    subject: str
    name: str
