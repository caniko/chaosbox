"""Short live stack smoke tests.

These tests intentionally require real model access and a PyTorch GPU backend,
so the default pytest configuration excludes them with ``-m not integration``.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.conftest import requires_gpu_and_token, requires_pyannote_audio_decoder

pytestmark = pytest.mark.integration


@requires_gpu_and_token
@requires_pyannote_audio_decoder
def test_live_default_stack_smoke(
    bobby_smoke_clip: Path,
    tmp_path: Path,
) -> None:
    from canscribe.api import transcribe_file

    output_path = tmp_path / "transcript-smoke.txt"
    result = transcribe_file(
        bobby_smoke_clip,
        device="auto",
        output_path=output_path,
        min_segment_duration=0.2,
    )

    assert output_path.exists()
    assert result.output_path == output_path
    assert len(result.segments) >= 1
    assert result.backend_metadata["asr_backend"] == "parakeet"
    assert result.backend_metadata["diarization_backend"] == "pyannote-community"
    assert result.backend_metadata["device"] in {"cuda", "mps", "cpu"}
    assert output_path.read_text(encoding="utf-8").strip()
