from __future__ import annotations

from canscribe.audio import get_audio_path


def test_get_audio_path_returns_audio_file_without_temp_file() -> None:
    assert get_audio_path("/tmp/sample.wav") == ("/tmp/sample.wav", None)


def test_get_audio_path_extracts_video_audio(monkeypatch) -> None:
    calls: list[str] = []

    def fake_extract(video_path: str) -> str:
        calls.append(video_path)
        return "/tmp/extracted.wav"

    monkeypatch.setattr("canscribe.audio.extract_audio_from_video", fake_extract)

    assert get_audio_path("/tmp/sample.mp4") == (
        "/tmp/extracted.wav",
        "/tmp/extracted.wav",
    )
    assert calls == ["/tmp/sample.mp4"]
