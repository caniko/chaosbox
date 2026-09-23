"""Integration tests using bobby.mp4 clip (7:31-9:24).

These tests validate the transcription pipeline with a real video clip
containing 3 speakers with background laughing.
"""

import re
import shutil
from pathlib import Path

import pytest

from tests.conftest import requires_gpu_and_token, requires_pyannote_audio_decoder

pytestmark = [pytest.mark.integration, pytest.mark.slow]


@requires_gpu_and_token
@requires_pyannote_audio_decoder
def test_transcribe_bobby_clip(bobby_clip: Path, tmp_path: Path) -> None:
    """Run the model once and verify transcript content, speakers, and coverage."""
    from canscribe.api import transcribe_file

    test_clip = tmp_path / "bobby_clip.mp4"
    shutil.copy(bobby_clip, test_clip)

    transcript_path = tmp_path / "transcript-bobby_clip.txt"
    transcribe_file(
        test_clip,
        asr_backend="moonshine",
        speaker_count=3,
        device="auto",
        output_path=transcript_path,
        debug=False,
    )

    assert transcript_path.exists(), f"Transcript file not created at {transcript_path}"
    content = transcript_path.read_text()
    assert content, "Transcript file is empty"

    lines = content.strip().split("\n")
    timestamp_pattern = re.compile(r"\[\d+\.\d+s - \d+\.\d+s\] SPEAKER_\d+: .+")
    valid_lines = [line for line in lines if timestamp_pattern.match(line)]
    assert valid_lines, f"No valid transcript lines found. Sample line: {lines[0]}"

    speaker_pattern = re.compile(r"SPEAKER_(\d+)")
    speakers = set(speaker_pattern.findall(content))
    assert len(speakers) == 3, f"Expected 3 speakers, found {speakers}"

    timestamp_pattern = re.compile(r"\[(\d+\.\d+)s - (\d+\.\d+)s\]")
    matches = timestamp_pattern.findall(content)

    assert matches, "No timestamp matches found in transcript"

    total_duration = sum(float(end) - float(start) for start, end in matches)

    min_expected_duration = 113 * 0.30  # 30% of clip
    assert total_duration >= min_expected_duration, (
        f"Transcribed duration {total_duration:.1f}s is less than "
        f"expected minimum {min_expected_duration:.1f}s (30% of 113s clip)"
    )

    last_end = max(float(end) for _, end in matches)
    assert last_end > 50, (
        f"Last segment ends at {last_end:.1f}s, expected segments to span "
        f"beyond 50s of the 113s clip"
    )
