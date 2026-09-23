from __future__ import annotations

import sys
from contextlib import nullcontext
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any
from unittest.mock import Mock

import pytest
import torch

from canscribe.backends import PyannoteCommunityBackend
from canscribe.pipeline import TranscriptionPipeline
from canscribe.types import (
    AsrResult,
    BackendSetupError,
    DiarizationSegment,
    TranscriptionRequest,
)


class FakeAsrBackend:
    name = "fake-asr"
    model_name = "fake-asr-model"

    def transcribe_chunk(
        self, audio_chunk: torch.Tensor, sample_rate: int
    ) -> AsrResult:
        return AsrResult(text="hello")


class FakeDiarizationBackend:
    name = "fake-diarization"
    model_name = "fake-diarization-model"

    def __init__(self) -> None:
        self.calls: list[dict[str, int | str | None]] = []

    def diarize(
        self,
        audio_path: str,
        *,
        speaker_count: int | None = None,
        min_speakers: int | None = None,
        max_speakers: int | None = None,
    ) -> tuple[list[DiarizationSegment], tuple[str, ...]]:
        self.calls.append(
            {
                "audio_path": audio_path,
                "speaker_count": speaker_count,
                "min_speakers": min_speakers,
                "max_speakers": max_speakers,
            }
        )
        return [DiarizationSegment(0.0, 1.0, "SPEAKER_00")], ()


@pytest.mark.parametrize(
    ("request_options", "expected_call", "expected_metadata"),
    [
        (
            {"speaker_count": 3},
            {"speaker_count": 3, "min_speakers": None, "max_speakers": None},
            {"speaker_count": "3"},
        ),
        (
            {"min_speakers": 2, "max_speakers": 5},
            {"speaker_count": None, "min_speakers": 2, "max_speakers": 5},
            {"min_speakers": "2", "max_speakers": "5"},
        ),
        (
            {},
            {"speaker_count": None, "min_speakers": None, "max_speakers": None},
            {},
        ),
    ],
)
def test_pipeline_forwards_speaker_options(
    monkeypatch,
    tmp_path: Path,
    request_options: dict[str, Any],
    expected_call: dict[str, int | None],
    expected_metadata: dict[str, str],
) -> None:
    diarization_backend = FakeDiarizationBackend()
    _patch_pipeline_audio(monkeypatch, tmp_path)

    result = TranscriptionPipeline(
        asr_backend=FakeAsrBackend(),
        diarization_backend=diarization_backend,
    ).run(TranscriptionRequest(tmp_path / "meeting.wav", **request_options))

    assert diarization_backend.calls == [
        {"audio_path": str(tmp_path / "meeting.wav"), **expected_call}
    ]
    assert {
        key: result.backend_metadata[key] for key in expected_metadata
    } == expected_metadata
    assert (
        set(result.backend_metadata) & {"speaker_count", "min_speakers", "max_speakers"}
    ) == set(expected_metadata)


@pytest.mark.parametrize(
    ("transcription_request", "message"),
    [
        (TranscriptionRequest("meeting.wav", speaker_count=0), "positive integer"),
        (TranscriptionRequest("meeting.wav", min_speakers=0), "positive integer"),
        (
            TranscriptionRequest("meeting.wav", speaker_count=2, max_speakers=4),
            "cannot be combined",
        ),
        (
            TranscriptionRequest("meeting.wav", min_speakers=5, max_speakers=2),
            "less than or equal",
        ),
    ],
)
def test_pipeline_rejects_invalid_speaker_counts(
    transcription_request: TranscriptionRequest,
    message: str,
) -> None:
    with pytest.raises(BackendSetupError, match=message):
        TranscriptionPipeline(
            asr_backend=FakeAsrBackend(),
            diarization_backend=FakeDiarizationBackend(),
        ).run(transcription_request)


@pytest.mark.parametrize(
    ("request_options", "expected_options"),
    [({"speaker_count": 4}, {"num_speakers": 4}), ({}, {})],
)
def test_pyannote_backend_forwards_speaker_options(
    monkeypatch,
    request_options: dict[str, Any],
    expected_options: dict[str, int],
) -> None:
    _install_fake_progress_hook(monkeypatch)
    monkeypatch.setattr(
        "canscribe.backends._load_audio_tensor",
        lambda audio_path: (torch.zeros(1, 16000), 16000),
    )

    timeline = SimpleNamespace(
        itertracks=lambda *, yield_label: [
            (SimpleNamespace(start=0.0, end=1.0), None, "SPEAKER_00")
        ]
    )
    pipeline = Mock(
        return_value=SimpleNamespace(exclusive_speaker_diarization=timeline)
    )
    backend = PyannoteCommunityBackend(device="cpu")
    backend._pipeline = pipeline  # type: ignore[attr-defined]

    segments, warnings = backend.diarize("meeting.wav", **request_options)

    assert warnings == ()
    assert segments == [DiarizationSegment(0.0, 1.0, "SPEAKER_00")]
    assert {
        key: value for key, value in pipeline.call_args.kwargs.items() if key != "hook"
    } == expected_options


def _patch_pipeline_audio(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(
        "canscribe.pipeline.get_audio_path",
        lambda path: (str(tmp_path / "meeting.wav"), None),
    )
    monkeypatch.setattr(
        "canscribe.pipeline.load_mono_audio",
        lambda path: (torch.ones(16000), 16000),
    )


def _install_fake_progress_hook(monkeypatch: pytest.MonkeyPatch) -> None:
    module_names = [
        "pyannote",
        "pyannote.audio",
        "pyannote.audio.pipelines",
        "pyannote.audio.pipelines.utils",
        "pyannote.audio.pipelines.utils.hook",
    ]
    for module_name in module_names:
        monkeypatch.setitem(sys.modules, module_name, ModuleType(module_name))

    hook_module = sys.modules["pyannote.audio.pipelines.utils.hook"]
    setattr(hook_module, "ProgressHook", nullcontext)
