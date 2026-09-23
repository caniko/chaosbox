# Diagnostics

Run diagnostics with:

```sh
canscribe check
```

To include an FFmpeg video decode probe, pass a video file:

```sh
canscribe check ~/Videos/meeting.mp4
```

The check command reports Python package imports, Torch device availability, FFmpeg availability, decoder behavior, pyannote import readiness, and torchcodec audio decoding.

Run the deeper library-path and GPU-runtime checks with:

```sh
canscribe doctor
canscribe doctor --verbose
```

## Common Failure Points

- `HF_TOKEN` is missing or the pyannote model terms have not been accepted.
- FFmpeg is unavailable outside the Nix shell.
- The selected uv platform extra does not match the machine.
- ROCm runtime libraries from the system shadow the PyTorch wheel runtime.
