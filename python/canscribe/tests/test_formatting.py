from __future__ import annotations

from pathlib import Path

from canscribe.formatting import (
    clean_repetitive_text,
    default_transcript_path,
)
from canscribe.types import TranscriptSegment


def test_transcript_segment_renders_visual_context() -> None:
    segment = TranscriptSegment(
        start=1.234,
        end=5.678,
        speaker="SPEAKER_00",
        text="hello there",
        face_id="Person A",
        emotion="composed",
        visual_description="slides with a title",
    )

    assert segment.to_text_lines() == [
        "[1.23s - 5.68s] SPEAKER_00 (Person A, composed): hello there",
        "    [Visual: slides with a title]",
    ]


def test_clean_repetitive_text_reduces_common_asr_artifacts() -> None:
    assert clean_repetitive_text("yes yes yes yes yes") == "yes yes yes"
    assert (
        clean_repetitive_text("hello, hello, hello, hello, hello, next")
        == "hello, hello, hello, next"
    )


def test_default_transcript_path_uses_input_stem() -> None:
    assert default_transcript_path(Path("/tmp/meeting.mp4")) == Path(
        "/tmp/transcript-meeting.txt"
    )
