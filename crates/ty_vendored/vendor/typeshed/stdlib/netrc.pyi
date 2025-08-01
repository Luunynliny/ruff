"""
An object-oriented interface to .netrc files.
"""

import sys
from _typeshed import StrOrBytesPath
from typing_extensions import TypeAlias

__all__ = ["netrc", "NetrcParseError"]

class NetrcParseError(Exception):
    """Exception raised on syntax errors in the .netrc file."""

    filename: str | None
    lineno: int | None
    msg: str
    def __init__(self, msg: str, filename: StrOrBytesPath | None = None, lineno: int | None = None) -> None: ...

# (login, account, password) tuple
if sys.version_info >= (3, 11):
    _NetrcTuple: TypeAlias = tuple[str, str, str]
else:
    _NetrcTuple: TypeAlias = tuple[str, str | None, str | None]

class netrc:
    hosts: dict[str, _NetrcTuple]
    macros: dict[str, list[str]]
    def __init__(self, file: StrOrBytesPath | None = None) -> None: ...
    def authenticators(self, host: str) -> _NetrcTuple | None:
        """Return a (user, account, password) tuple for given host."""
