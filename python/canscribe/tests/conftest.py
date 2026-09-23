"""Pytest fixtures for canscribe integration tests."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

# Path to the test video in the project root
BOBBY_VIDEO = Path(__file__).parent.parent / "bobby.mp4"

# Clip timing: 7:31-9:24 in seconds
CLIP_START_SECONDS = 7 * 60 + 31  # 451 seconds
CLIP_DURATION_SECONDS = (9 * 60 + 24) - CLIP_START_SECONDS  # 113 seconds
SMOKE_CLIP_DURATION_SECONDS = 12


def has_torch_gpu() -> bool:
    """Check if PyTorch exposes a GPU device.

    ROCm/HIP builds expose AMD GPUs through the torch.cuda API, so this covers
    both CUDA and ROCm backends.
    """
    try:
        import torch

        return torch.cuda.is_available()
    except Exception:
        return False


def has_hf_token() -> bool:
    """Check if HuggingFace token is set."""
    return bool(os.environ.get("HF_TOKEN"))


def pyannote_audio_decoder_works() -> bool:
    """Check if pyannote.audio's AudioDecoder is properly available.

    pyannote.audio 4.x has a bug where AudioDecoder may not be defined
    if torchcodec/FFmpeg libraries are not properly installed.
    """
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import pyannote.audio.core.io as io_module; "
                "raise SystemExit(0 if hasattr(io_module, 'AudioDecoder') else 1)"
            ),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
        timeout=30,
    )
    return probe.returncode == 0


requires_gpu_and_token = pytest.mark.skipif(
    not has_torch_gpu() or not has_hf_token(),
    reason="Requires PyTorch GPU backend and HF_TOKEN",
)

requires_pyannote_audio_decoder = pytest.mark.skipif(
    not pyannote_audio_decoder_works(),
    reason="pyannote.audio AudioDecoder not available (torchcodec/FFmpeg issue)",
)


@pytest.fixture(scope="session")
def bobby_video_path() -> Path:
    """Return path to the bobby.mp4 video file."""
    if not BOBBY_VIDEO.exists():
        pytest.skip(f"Test video not found: {BOBBY_VIDEO}")
    return BOBBY_VIDEO


@pytest.fixture(scope="session")
def bobby_clip(bobby_video_path: Path, tmp_path_factory) -> Path:
    """
    Extract a 113-second clip (7:31-9:24) from bobby.mp4 to a temporary file.

    Uses ffmpeg to extract the clip with these parameters:
    - Start: 451 seconds (7:31)
    - Duration: 113 seconds (until 9:24)

    The clip is cached for the entire test session.
    """
    clip_path = tmp_path_factory.mktemp("canscribe") / "bobby_clip.mp4"
    _extract_clip(bobby_video_path, clip_path, CLIP_DURATION_SECONDS)
    return clip_path


@pytest.fixture
def bobby_smoke_clip(bobby_video_path: Path, tmp_path: Path) -> Path:
    """Extract a short clip for live model smoke tests."""
    clip_path = tmp_path / "bobby_smoke_clip.mp4"
    _extract_clip(bobby_video_path, clip_path, SMOKE_CLIP_DURATION_SECONDS)
    return clip_path


def _extract_clip(source: Path, destination: Path, duration: int) -> None:
    subprocess.run(
        [
            "ffmpeg",
            "-y",
            "-ss",
            str(CLIP_START_SECONDS),
            "-i",
            str(source),
            "-t",
            str(duration),
            "-c",
            "copy",
            str(destination),
        ],
        capture_output=True,
        text=True,
        timeout=60,
        check=True,
    )
    if not destination.exists():
        pytest.fail(f"Clip file not created: {destination}")
