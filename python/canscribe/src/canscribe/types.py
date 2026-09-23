from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

AsrBackendName = Literal["parakeet", "moonshine"]
DevicePolicy = Literal["auto", "cpu", "cuda", "mps"]


@dataclass(frozen=True)
class TranscriptSegment:
    """One speaker-attributed transcript segment."""

    start: float
    end: float
    speaker: str
    text: str
    face_id: str | None = None
    emotion: str | None = None
    visual_description: str | None = None

    @property
    def speaker_label(self) -> str:
        if self.face_id and self.emotion:
            return f"{self.speaker} ({self.face_id}, {self.emotion})"
        if self.face_id:
            return f"{self.speaker} ({self.face_id})"
        return self.speaker

    def to_text_lines(self) -> list[str]:
        lines = [
            f"[{self.start:.2f}s - {self.end:.2f}s] {self.speaker_label}: {self.text}"
        ]
        if self.visual_description:
            lines.append(f"    [Visual: {self.visual_description}]")
        return lines


@dataclass(frozen=True)
class TranscriptionRequest:
    """Library request object for file transcription."""

    input_path: Path | str
    asr_backend: AsrBackendName = "parakeet"
    asr_model: str | None = None
    diarization_model: str | None = None
    speaker_count: int | None = None
    min_speakers: int | None = None
    max_speakers: int | None = None
    device: DevicePolicy = "auto"
    visual: bool = False
    output_path: Path | str | None = None
    debug: bool = False
    resume: bool = False
    min_segment_duration: float = 0.2


@dataclass(frozen=True)
class TranscriptionResult:
    """Result returned by the library API."""

    segments: tuple[TranscriptSegment, ...]
    output_path: Path
    backend_metadata: dict[str, str] = field(default_factory=dict)
    warnings: tuple[str, ...] = ()


@dataclass(frozen=True)
class DiarizationSegment:
    """Speaker segment emitted by diarization backends."""

    start: float
    end: float
    speaker: str


@dataclass(frozen=True)
class AsrResult:
    """Text emitted by an ASR backend for one audio chunk."""

    text: str


class BackendSetupError(RuntimeError):
    """A requested backend cannot be initialized or used."""
