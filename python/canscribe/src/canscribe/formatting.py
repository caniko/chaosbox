from __future__ import annotations

import re
from pathlib import Path
from typing import IO

from .types import TranscriptSegment


_END_TIME_RE = re.compile(r"^\[[\d.]+s - ([\d.]+)s\]")


def read_transcript_end_time(path: Path) -> float:
    """Parse an existing transcript and return the last segment's end time.

    Returns 0.0 if the file doesn't exist or has no segments.
    """
    if not path.exists():
        return 0.0

    last_time = 0.0
    with path.open("r", encoding="utf-8") as f:
        for line in f:
            m = _END_TIME_RE.match(line)
            if m:
                try:
                    last_time = float(m.group(1))
                except ValueError:
                    pass
    return last_time


def clean_repetitive_text(text: str, max_repeats: int = 3) -> str:
    """Reduce common repeated-word artifacts from ASR output."""
    if not text:
        return text

    def reduce_comma_repeats(match: re.Match[str]) -> str:
        unit = match.group(1)
        return ", ".join([unit] * max_repeats)

    comma_pattern = r"\b(\w+)(?:,\s*\1){" + str(max_repeats) + r",}\b"
    text = re.sub(comma_pattern, reduce_comma_repeats, text, flags=re.IGNORECASE)

    def reduce_space_repeats(match: re.Match[str]) -> str:
        unit = match.group(1)
        return " ".join([unit.strip()] * max_repeats)

    space_pattern = r"\b(\w+)(?:\s+\1){" + str(max_repeats) + r",}\b"
    text = re.sub(space_pattern, reduce_space_repeats, text, flags=re.IGNORECASE)
    text = re.sub(r"\s{2,}", " ", text)

    return text.strip()


def default_transcript_path(input_path: Path) -> Path:
    return input_path.parent / f"transcript-{input_path.stem}.txt"


def append_segment(
    handle: IO[str], segment: TranscriptSegment, debug: bool = False
) -> None:
    """Write a single segment to an already-open file handle."""
    for line in segment.to_text_lines():
        rendered = f"{line}\n"
        handle.write(rendered)
        if debug:
            print(rendered, end="")
