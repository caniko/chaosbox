from __future__ import annotations

from pathlib import Path
from typing import Annotated, Optional

import torch
import typer
from rich.console import Console
from rich.table import Table

from .api import transcribe_file
from .config import (
    DEFAULT_ASR_BACKEND,
    MOONSHINE_MODEL,
    PARAKEET_MODEL,
    PYANNOTE_MODEL,
    silence_stderr,
    supports_flash_attention,
)
from .types import AsrBackendName, BackendSetupError

app = typer.Typer(
    name="canscribe",
    help="Audio/video transcription with local diarization and modern ASR backends.",
    no_args_is_help=True,
)

console = Console()


@app.command()
def transcribe(
    audio_file: Annotated[
        Path,
        typer.Argument(
            help="Path to the audio or video file to transcribe",
            exists=True,
            readable=True,
        ),
    ],
    asr_backend: Annotated[
        AsrBackendName,
        typer.Option("--asr", help="ASR backend to use: parakeet or moonshine"),
    ] = DEFAULT_ASR_BACKEND,  # type: ignore[assignment]
    asr_model: Annotated[
        Optional[str],
        typer.Option("--model", "-m", help="ASR model name. Defaults depend on --asr."),
    ] = None,
    diarization_model: Annotated[
        str,
        typer.Option("--diarization", "-d", help="Pyannote diarization model"),
    ] = PYANNOTE_MODEL,
    speaker_count: Annotated[
        Optional[int],
        typer.Option(
            "--speakers",
            help="Exact number of call participants/speakers to diarize",
            min=1,
        ),
    ] = None,
    min_speakers: Annotated[
        Optional[int],
        typer.Option(
            "--min-speakers",
            help="Minimum number of call participants/speakers to diarize",
            min=1,
        ),
    ] = None,
    max_speakers: Annotated[
        Optional[int],
        typer.Option(
            "--max-speakers",
            help="Maximum number of call participants/speakers to diarize",
            min=1,
        ),
    ] = None,
    cpu: Annotated[
        bool,
        typer.Option("--cpu", help="Force CPU usage instead of auto GPU detection"),
    ] = False,
    visual: Annotated[
        bool,
        typer.Option("--visual", "-v", help="Enable visual analysis for video inputs"),
    ] = False,
    debug: Annotated[
        bool,
        typer.Option("--debug", help="Print transcript lines as they are written"),
    ] = False,
    resume: Annotated[
        bool,
        typer.Option(
            "--resume",
            "-r",
            help="Resume a partial transcript from existing output file",
        ),
    ] = False,
) -> None:
    """Transcribe an audio or video file with speaker diarization."""
    device = "cpu" if cpu else "auto"

    try:
        result = transcribe_file(
            audio_file,
            asr_backend=asr_backend,
            asr_model=asr_model,
            diarization_model=diarization_model,
            speaker_count=speaker_count,
            min_speakers=min_speakers,
            max_speakers=max_speakers,
            device=device,
            visual=visual,
            debug=debug,
            resume=resume,
        )
    except BackendSetupError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(1) from exc

    for warning in result.warnings:
        typer.echo(f"Warning: {warning}", err=True)
    typer.echo(f"Transcript saved to: {result.output_path}")


@app.command()
def check(
    video_file: Annotated[
        Optional[Path],
        typer.Argument(
            help="Optional video file to test hardware-accelerated decoding",
            exists=True,
            readable=True,
        ),
    ] = None,
    models: Annotated[
        bool,
        typer.Option("--models", help="Also validate configured model backends"),
    ] = False,
) -> None:
    """Run diagnostic checks on system components."""
    import os
    import shutil
    import subprocess
    import sys

    table = Table(title="System Diagnostics")
    table.add_column("Component", style="cyan")
    table.add_column("Status", style="green")
    table.add_column("Details", style="dim")

    all_ok = True
    table.add_row("Python", "OK", sys.version.split()[0])
    table.add_row("PyTorch", "OK", torch.__version__)

    with silence_stderr():
        cuda_available = torch.cuda.is_available()
    if cuda_available:
        gpu_name = torch.cuda.get_device_name(0)
        props = torch.cuda.get_device_properties(0)
        major, minor = torch.cuda.get_device_capability()
        hip_version = getattr(torch.version, "hip", None)
        cuda_version = torch.version.cuda or hip_version or "N/A"  # type: ignore[attr-defined]
        backend_name = "ROCm/HIP" if hip_version else "CUDA"
        arch = getattr(props, "gcnArchName", f"CC {major}.{minor}")
        memory_gib = props.total_memory / 1024**3
        table.add_row(
            backend_name,
            "OK",
            f"{cuda_version} ({gpu_name}, {arch}, {memory_gib:.1f} GiB)",
        )
    elif torch.backends.mps.is_available():
        table.add_row("MPS", "OK", "Metal Performance Shaders available")
    else:
        table.add_row("GPU", "WARN", "No GPU available; CPU mode only")

    attention_status = "OK" if supports_flash_attention() else "WARN"
    attention_detail = (
        "available" if attention_status == "OK" else "not available or unsupported"
    )
    table.add_row("Flash Attention", attention_status, attention_detail)

    ffmpeg_path = shutil.which("ffmpeg")
    if ffmpeg_path:
        try:
            result = subprocess.run(
                ["ffmpeg", "-version"],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            version_line = result.stdout.split("\n")[0] if result.stdout else "unknown"
            table.add_row(
                "FFmpeg", "OK", version_line.replace("ffmpeg version ", "")[:50]
            )
        except Exception as exc:
            table.add_row("FFmpeg", "WARN", str(exc))
    else:
        table.add_row("FFmpeg", "FAIL", "Not found in PATH")
        all_ok = False

    if models:
        if os.environ.get("HF_TOKEN"):
            table.add_row("HF_TOKEN", "OK", "set")
        else:
            table.add_row(
                "HF_TOKEN",
                "FAIL",
                "required for pyannote/speaker-diarization-community-1",
            )
            all_ok = False

        try:
            import transformers

            table.add_row("Transformers", "OK", transformers.__version__)
            table.add_row("Parakeet ASR", "OK", PARAKEET_MODEL)
        except Exception as exc:
            table.add_row("Parakeet ASR", "FAIL", str(exc))
            all_ok = False

        try:
            from transformers import MoonshineForConditionalGeneration  # noqa: F401

            table.add_row("Moonshine ASR", "OK", MOONSHINE_MODEL)
        except Exception as exc:
            table.add_row("Moonshine ASR", "WARN", str(exc))

        pyannote_ok, pyannote_detail = _check_pyannote_audio_import()
        table.add_row(
            "Pyannote Audio",
            "OK" if pyannote_ok else "FAIL",
            pyannote_detail,
        )
        if not pyannote_ok:
            all_ok = False

        torchcodec_ok, torchcodec_detail = _check_torchcodec_audio_decoder()
        table.add_row(
            "TorchCodec",
            "OK" if torchcodec_ok else "FAIL",
            torchcodec_detail,
        )
        if not torchcodec_ok:
            all_ok = False

    _add_ffmpeg_video_test(table, video_file)
    console.print(table)

    if not all_ok:
        raise typer.Exit(1)


@app.command()
def doctor(
    verbose: Annotated[
        bool,
        typer.Option("--verbose", "-v", help="Show explanations and fix suggestions"),
    ] = False,
    probe: Annotated[
        bool,
        typer.Option(
            "--probe",
            "-p",
            help="Run a GPU kernel probe to check runtime behavior (requires torch + GPU)",
        ),
    ] = False,
) -> None:
    """Run deep environment diagnostics (library paths, env vars, GPU libs)."""
    from .checks.main import print_results, run_checks

    results = run_checks(probe=probe)
    print_results(results, verbose=verbose)
    has_fail = any(r.status == "FAIL" for r in results)
    if has_fail:
        raise typer.Exit(1)


def _check_torchcodec_audio_decoder() -> tuple[bool, str]:
    """Verify that pyannote can use TorchCodec with the active FFmpeg ABI."""
    import glob
    import importlib.metadata as metadata
    import os
    import subprocess
    import sys

    avutil_candidates: list[str] = []
    for library_path in os.environ.get("LD_LIBRARY_PATH", "").split(":"):
        if library_path:
            avutil_candidates.extend(glob.glob(f"{library_path}/libavutil.so.*"))
    abi_detail = ", ".join(sorted(os.path.basename(path) for path in avutil_candidates))
    if not abi_detail:
        abi_detail = "no libavutil.so.* found in LD_LIBRARY_PATH"

    try:
        torchcodec_version = metadata.version("torchcodec")
    except metadata.PackageNotFoundError:
        return False, f"torchcodec not installed ({abi_detail})"

    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import pyannote.audio.core.io as io_module; "
                "raise SystemExit(0 if hasattr(io_module, 'AudioDecoder') else 1)"
            ),
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )
    if probe.returncode != 0:
        detail = (probe.stderr or probe.stdout).strip().splitlines()
        message = detail[-1] if detail else f"probe exited {probe.returncode}"
        return (
            False,
            f"torchcodec {torchcodec_version}; decoder probe failed: {message}; {abi_detail}",
        )

    return True, f"torchcodec {torchcodec_version}; {abi_detail}"


def _check_pyannote_audio_import() -> tuple[bool, str]:
    """Import pyannote.audio in a child process because TorchCodec can abort."""
    import subprocess
    import sys

    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            "import pyannote.audio; print(pyannote.audio.__version__)",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )
    if probe.returncode == 0:
        return True, probe.stdout.strip() or "imported"

    detail = (probe.stderr or probe.stdout).strip().splitlines()
    return False, detail[-1] if detail else f"probe exited {probe.returncode}"


def _add_ffmpeg_video_test(table: Table, video_file: Path | None) -> None:
    if video_file is None:
        return

    import subprocess

    probe_result = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,duration",
            "-of",
            "csv=p=0",
            str(video_file),
        ],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    if probe_result.returncode == 0:
        table.add_row("Video Probe", "OK", probe_result.stdout.strip())
    else:
        table.add_row("Video Probe", "WARN", probe_result.stderr.strip()[:80])

    decode_result = subprocess.run(
        [
            "ffmpeg",
            "-hwaccel",
            "auto",
            "-i",
            str(video_file),
            "-t",
            "1",
            "-f",
            "null",
            "-",
        ],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if decode_result.returncode == 0:
        table.add_row("Video Decode", "OK", "1 second decoded")
    else:
        table.add_row("Video Decode", "FAIL", decode_result.stderr.strip()[:80])


def main() -> None:
    app()


if __name__ == "__main__":
    main()
