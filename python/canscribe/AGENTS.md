# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## Build and Development Commands

```bash
# Enter dev environment (installs deps, activates venv)
nix develop

# Sync dependencies (choose ONE platform extra)
uv sync --extra nvidia        # NVIDIA CUDA 12.8
uv sync --extra amd           # AMD ROCm 6.4 (Linux only)
uv sync --extra apple         # Apple Silicon (MPS)

# Optional: Flash Attention for NVIDIA Ampere+ GPUs (CC 8.0+)
uv sync --extra nvidia --extra flash

# Run type checking
uv run mypy .

# Format code
uv run ruff format .

# Run the CLI
canscribe <audio-or-video-file>

# Run system diagnostics
canscribe check [video-file]
```

## Architecture Overview

This is an audio/video transcription CLI (`canscribe`, with `ct` as a compatibility alias) that combines speech-to-text with speaker diarization and optional visual analysis.

### Core Pipeline (`src/canscribe/`)

**Entry Point**: `main.py` - Typer CLI with two commands:

- `transcribe` (default): Main transcription with optional `--visual` mode
- `check`: System diagnostics for GPU, FFmpeg, decoders, and dependencies

**Audio Processing Flow**:

1. `audio.py` - Extracts audio from video via FFmpeg (hardware-accelerated)
2. `pipeline.py` and `backends.py` - Main transcription pipeline:
   - Pyannote diarization → speaker segments
   - Parakeet by default, Moonshine compatibility backend → text per segment
   - Streams results to `transcript-<filename>.txt`

**Visual Analysis Flow** (enabled with `--visual`):

1. `pipeline.py` - Extracts segment keyframes and attaches visual context
2. `vision/face_analysis.py` - InsightFace `buffalo_l` embeddings + HuggingFace emotion classification
3. `vision/scene_analysis.py` - Qwen3-VL-4B-Instruct scene descriptions

### Key Design Patterns

- **Lazy model loading**: All ML models are loaded on first use via `@property` accessors
- **Generator streaming**: Transcript segments are generated and saved incrementally
- **Flash Attention auto-detection**: `config.py:supports_flash_attention()` checks GPU capability and library availability
- **Safe globals for PyTorch 2.6+**: Pyannote classes added to `torch.serialization.add_safe_globals()`

### Models Used

| Component         | Model                                      | Purpose                                   |
| ----------------- | ------------------------------------------ | ----------------------------------------- |
| ASR               | `nvidia/parakeet-tdt-0.6b-v3`              | Default speech-to-text                    |
| ASR compatibility | `UsefulSensors/moonshine-tiny`             | Lightweight English speech-to-text        |
| Diarization       | `pyannote/speaker-diarization-community-1` | Speaker identification                    |
| Face Detection    | InsightFace `buffalo_l`                    | Face detection, landmarks, and embeddings |
| Emotion           | `dima806/facial_emotions_image_detection`  | Facial expression classification          |
| Scene VLM         | `Qwen/Qwen3-VL-4B-Instruct`                | Non-face frame descriptions               |

### Environment Requirements

- Python 3.13
- GPU recommended: NVIDIA (CUDA), AMD (ROCm), or Apple Silicon (MPS)
- FFmpeg with hardware acceleration support
- `HF_TOKEN` environment variable for pyannote model access
- OpenCV provided by Nix (opencv4Full with VAAPI)
