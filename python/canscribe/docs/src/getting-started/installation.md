# Installation

## Requirements

- Python 3.13
- FFmpeg for media decoding and video audio extraction
- A platform backend selected through one uv extra: `nvidia`, `amd`, `apple`, or `cpu`
- `HF_TOKEN` for pyannote diarization model access

GPU acceleration is recommended. The project defines platform extras for NVIDIA CUDA 12.8, AMD ROCm 6.4, Apple Silicon MPS, and CPU-only operation.

## Nix Development Shell

From the repository root:

```sh
nix develop
```

The development shell provides Python 3.13, uv, FFmpeg, OpenCV, and the native libraries needed by the Python wheels.

## Sync Python Dependencies

Choose one platform extra:

```sh
uv sync --extra nvidia
uv sync --extra amd
uv sync --extra apple
uv sync --extra cpu
```

For NVIDIA Ampere or newer GPUs where Flash Attention is available:

```sh
uv sync --extra nvidia --extra flash
```

## Hugging Face Token

Create a Hugging Face token, accept the model terms for `pyannote/speaker-diarization-community-1`, then export the token before running transcription:

```sh
export HF_TOKEN="hf_your_token_here"
```
