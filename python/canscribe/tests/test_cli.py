from __future__ import annotations

from pathlib import Path

from typer.testing import CliRunner

from canscribe.main import app
from canscribe.types import BackendSetupError, TranscriptionResult


runner = CliRunner()


def test_cli_prints_backend_setup_errors(tmp_path: Path, monkeypatch) -> None:
    audio_path = tmp_path / "sample.wav"
    audio_path.write_bytes(b"not a wav")

    def fake_transcribe_file(*args, **kwargs):
        raise BackendSetupError("backend unavailable")

    monkeypatch.setattr("canscribe.main.transcribe_file", fake_transcribe_file)

    result = runner.invoke(app, ["transcribe", str(audio_path)])

    assert result.exit_code == 1
    assert "backend unavailable" in result.stderr


def test_cli_success_prints_transcript_path(tmp_path: Path, monkeypatch) -> None:
    audio_path = tmp_path / "sample.wav"
    audio_path.write_bytes(b"not a wav")
    output_path = tmp_path / "transcript-sample.txt"

    def fake_transcribe_file(*args, **kwargs):
        return TranscriptionResult(
            segments=(),
            output_path=output_path,
            backend_metadata={"device": "cpu"},
        )

    monkeypatch.setattr("canscribe.main.transcribe_file", fake_transcribe_file)

    result = runner.invoke(app, ["transcribe", str(audio_path), "--cpu"])

    assert result.exit_code == 0
    assert f"Transcript saved to: {output_path}" in result.stdout


def test_cli_forwards_speaker_count_options(tmp_path: Path, monkeypatch) -> None:
    audio_path = tmp_path / "sample.wav"
    audio_path.write_bytes(b"not a wav")
    output_path = tmp_path / "transcript-sample.txt"
    calls = []

    def fake_transcribe_file(*args, **kwargs):
        calls.append(kwargs)
        return TranscriptionResult(
            segments=(),
            output_path=output_path,
            backend_metadata={"device": "cpu"},
        )

    monkeypatch.setattr("canscribe.main.transcribe_file", fake_transcribe_file)

    exact_result = runner.invoke(
        app, ["transcribe", str(audio_path), "--speakers", "4"]
    )
    range_result = runner.invoke(
        app,
        [
            "transcribe",
            str(audio_path),
            "--min-speakers",
            "2",
            "--max-speakers",
            "5",
        ],
    )

    assert exact_result.exit_code == 0
    assert range_result.exit_code == 0
    assert calls[0]["speaker_count"] == 4
    assert calls[0]["min_speakers"] is None
    assert calls[0]["max_speakers"] is None
    assert calls[1]["speaker_count"] is None
    assert calls[1]["min_speakers"] == 2
    assert calls[1]["max_speakers"] == 5
