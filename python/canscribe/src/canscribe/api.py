from __future__ import annotations

from pathlib import Path
from typing import Any

from .pipeline import TranscriptionPipeline
from .types import TranscriptionRequest, TranscriptionResult


def transcribe(request: TranscriptionRequest) -> TranscriptionResult:
    """Transcribe a file using the typed library request API."""
    return TranscriptionPipeline().run(request)


def transcribe_file(path: Path | str, **options: Any) -> TranscriptionResult:
    """Convenience wrapper for callers that do not need to build a request object."""
    return transcribe(TranscriptionRequest(input_path=path, **options))
