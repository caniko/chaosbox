# canscribe

Audio and video transcription CLI with speaker diarization, GPU backends, and optional visual context. The default ASR backend is NVIDIA Parakeet; Moonshine remains available as a compatibility backend.

```sh
nix develop
uv sync --extra nvidia
export HF_TOKEN="hf_your_token_here"
canscribe ~/Videos/meeting.mp4
```

The transcript is written next to the input media as `transcript-<filename>.txt`.

Read the [documentation](https://caniko.codeberg.page/canscribe/) for installation, platform setup, CLI options, diagnostics, visual analysis, and development workflows.

## Requirements

- Python 3.13
- FFmpeg
- One platform extra: `nvidia`, `amd`, `apple`, or `cpu`
- A Hugging Face token with access to `pyannote/speaker-diarization-community-1`

## License

MIT
