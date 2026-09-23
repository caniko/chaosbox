from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
import torch

from canscribe.pipeline import TranscriptionPipeline
from canscribe.types import (
    AsrResult,
    BackendSetupError,
    DiarizationSegment,
    TranscriptionRequest,
)


@dataclass
class FakeAsrBackend:
    model_name: str = "fake-asr"
    name: str = "fake-asr"
    calls: int = 0

    def transcribe_chunk(
        self, audio_chunk: torch.Tensor, sample_rate: int
    ) -> AsrResult:
        self.calls += 1
        return AsrResult(text=f"chunk {self.calls} at {sample_rate}hz")


@dataclass
class FakeDiarizationBackend:
    model_name: str = "fake-diarization"
    name: str = "fake-diarization"

    def diarize(
        self,
        audio_path: str,
        *,
        speaker_count: int | None = None,
        min_speakers: int | None = None,
        max_speakers: int | None = None,
    ) -> tuple[list[DiarizationSegment], tuple[str, ...]]:
        return (
            [
                DiarizationSegment(start=0.0, end=0.1, speaker="SPEAKER_SHORT"),
                DiarizationSegment(start=0.0, end=1.0, speaker="SPEAKER_00"),
                DiarizationSegment(start=1.0, end=2.0, speaker="SPEAKER_01"),
            ],
            ("non-fatal diarization warning",),
        )


def test_pipeline_writes_transcript_metadata_and_removes_temp_audio(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    input_path = tmp_path / "meeting.mp4"
    input_path.write_text("not really a video", encoding="utf-8")
    output_path = tmp_path / "result.txt"
    temp_audio = tmp_path / "temp.wav"
    temp_audio.write_text("temporary audio", encoding="utf-8")
    asr = FakeAsrBackend()

    monkeypatch.setattr(
        "canscribe.pipeline.get_audio_path",
        lambda path: (str(temp_audio), str(temp_audio)),
    )
    monkeypatch.setattr(
        "canscribe.pipeline.load_mono_audio",
        lambda path: (torch.ones(32000), 16000),
    )

    result = TranscriptionPipeline(
        asr_backend=asr,
        diarization_backend=FakeDiarizationBackend(),
    ).run(
        TranscriptionRequest(
            input_path=input_path,
            output_path=output_path,
            device="cpu",
            min_segment_duration=0.2,
        )
    )

    assert asr.calls == 2
    assert not temp_audio.exists()
    assert result.output_path == output_path
    assert result.warnings == ("non-fatal diarization warning",)
    assert result.backend_metadata == {
        "asr_backend": "fake-asr",
        "asr_model": "fake-asr",
        "diarization_backend": "fake-diarization",
        "diarization_model": "fake-diarization",
        "device": "cpu",
    }
    assert output_path.read_text(encoding="utf-8") == (
        "[0.00s - 1.00s] SPEAKER_00: chunk 1 at 16000hz\n"
        "[1.00s - 2.00s] SPEAKER_01: chunk 2 at 16000hz\n"
    )


def test_pipeline_rejects_visual_analysis_for_audio_input(tmp_path: Path) -> None:
    input_path = tmp_path / "meeting.wav"
    input_path.write_bytes(b"not a wav")

    with pytest.raises(BackendSetupError, match="Visual analysis requires a video"):
        TranscriptionPipeline(
            asr_backend=FakeAsrBackend(),
            diarization_backend=FakeDiarizationBackend(),
        ).run(TranscriptionRequest(input_path=input_path, visual=True, device="cpu"))


def test_pipeline_attaches_offline_visual_context(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    input_path = tmp_path / "meeting.mp4"
    input_path.write_text("not really a video", encoding="utf-8")
    output_path = tmp_path / "visual.txt"
    frame = torch.zeros(1).numpy()

    monkeypatch.setattr(
        "canscribe.pipeline.get_audio_path",
        lambda path: (str(input_path), None),
    )
    monkeypatch.setattr(
        "canscribe.pipeline.load_mono_audio",
        lambda path: (torch.ones(32000), 16000),
    )

    def fake_extract_frames(video_file: str, timestamps: list[float]):
        return {timestamps[0]: frame, timestamps[1]: frame}

    face_analyzer = Mock()
    face_analyzer.analyze_faces.side_effect = [
        [
            SimpleNamespace(
                face_id="Person A",
                emotion="composed",
                bbox=(0, 0, 20, 30),
            )
        ],
        [],
    ]
    scene_analyzer = Mock()
    scene_analyzer.describe_frame.return_value = "scene at 1.5s"

    monkeypatch.setattr(
        "canscribe.vision.extract_frames_at_timestamps", fake_extract_frames
    )
    monkeypatch.setattr("canscribe.vision.FaceAnalyzer", lambda device: face_analyzer)
    monkeypatch.setattr("canscribe.vision.SceneAnalyzer", lambda device: scene_analyzer)

    result = TranscriptionPipeline(
        asr_backend=FakeAsrBackend(),
        diarization_backend=FakeDiarizationBackend(),
    ).run(
        TranscriptionRequest(
            input_path=input_path,
            output_path=output_path,
            visual=True,
            device="cpu",
            min_segment_duration=0.2,
        )
    )

    assert result.segments[0].face_id == "Person A"
    assert result.segments[0].emotion == "composed"
    assert result.segments[1].visual_description == "scene at 1.5s"
    assert output_path.read_text(encoding="utf-8") == (
        "[0.00s - 1.00s] SPEAKER_00 (Person A, composed): chunk 1 at 16000hz\n"
        "[1.00s - 2.00s] SPEAKER_01: chunk 2 at 16000hz\n"
        "    [Visual: scene at 1.5s]\n"
    )
