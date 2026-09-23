import os
from contextlib import contextmanager

import torch

VIDEO_EXTENSIONS = {".mp4", ".mkv", ".avi", ".mov", ".webm", ".flv", ".wmv"}

PARAKEET_MODEL = "nvidia/parakeet-tdt-0.6b-v3"
MOONSHINE_MODEL = "UsefulSensors/moonshine-tiny"
PYANNOTE_MODEL = "pyannote/speaker-diarization-community-1"
QWEN3_VL_MODEL = "Qwen/Qwen3-VL-4B-Instruct"

ASR_MAX_NEW_TOKENS = 512

DEFAULT_ASR_BACKEND = "parakeet"


@contextmanager
def silence_stderr():
    """Temporarily redirect C-level stderr to /dev/null.

    The ROCm-bundled libdrm_amdgpu.so prints a harmless but noisy
    message to stderr when it fails to open its hardcoded path
    /opt/amdgpu/share/libdrm/amdgpu.ids on Nix systems.
    This silences it during the first GPU probe, which is the only
    time the message appears.
    """
    devnull = os.open(os.devnull, os.O_WRONLY)
    saved = os.dup(2)
    os.dup2(devnull, 2)
    os.close(devnull)
    try:
        yield
    finally:
        os.dup2(saved, 2)
        os.close(saved)


def supports_flash_attention() -> bool:
    """
    Auto-detect if Flash Attention 2 is supported.

    Requirements:
    - CUDA GPU available
    - Compute Capability 8.0+ (Ampere: RTX 30xx, 40xx, A100, H100)
    - flash_attn library installed

    Returns:
        True if Flash Attention 2 can be used, False otherwise.
    """
    # Check if we have a GPU
    with silence_stderr():
        if not torch.cuda.is_available():
            return False

    # Check GPU architecture (Ampere or newer required)
    # Flash Attn 2 requires Compute Capability 8.0+
    major, _ = torch.cuda.get_device_capability()
    if major < 8:
        return False

    # Check if the library is actually installed
    try:
        import flash_attn  # type: ignore[import-not-found]  # noqa: F401

        return True
    except ImportError:
        return False


def get_attention_config(device: str) -> tuple[str, torch.dtype]:
    """Return a conservative attention implementation and dtype for Transformers."""
    if device == "cuda" and supports_flash_attention():
        return "flash_attention_2", torch.float16
    if device in {"cuda", "mps"}:
        return "sdpa", torch.float16
    return "sdpa", torch.float32


def get_device() -> str:
    """
    Auto-detect best available compute device.

    Priority:
    1. CUDA (NVIDIA GPUs)
    2. MPS (Apple Silicon)
    3. CPU (fallback)

    Returns:
        Device string: "cuda", "mps", or "cpu"
    """
    with silence_stderr():
        if torch.cuda.is_available():
            return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"
