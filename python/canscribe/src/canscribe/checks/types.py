from typing import Literal, NamedTuple


Status = Literal["OK", "WARN", "FAIL", "INFO"]


class CheckResult(NamedTuple):
    status: Status
    name: str
    detail: str
    explanation: str | None = None
    fix: str | None = None
