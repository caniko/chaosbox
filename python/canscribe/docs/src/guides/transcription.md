# Transcription

The normal pipeline extracts audio when needed, runs pyannote diarization, transcribes each speaker segment, and writes transcript lines incrementally.

## Basic Usage

```sh
canscribe [OPTIONS] AUDIO_FILE
```

Supported video inputs include `mp4`, `mkv`, `avi`, `mov`, `webm`, `flv`, and `wmv`. Audio-only inputs are passed directly to the transcription path.

## Common Options

| Option                | Description                             | Default                                    |
| --------------------- | --------------------------------------- | ------------------------------------------ |
| `--asr`               | ASR backend: `parakeet` or `moonshine`  | `parakeet`                                 |
| `--model`, `-m`       | ASR model identifier                    | backend default                            |
| `--diarization`, `-d` | pyannote diarization model              | `pyannote/speaker-diarization-community-1` |
| `--speakers`          | Exact speaker count                     | auto                                       |
| `--min-speakers`      | Minimum speaker count                   | auto                                       |
| `--max-speakers`      | Maximum speaker count                   | auto                                       |
| `--cpu`               | Force CPU execution                     | false                                      |
| `--debug`             | Print each segment while processing     | false                                      |
| `--visual`, `-v`      | Enable visual analysis for video inputs | false                                      |
| `--resume`, `-r`      | Resume an existing partial transcript   | false                                      |

## Transcript Format

```text
[0.00s - 2.34s] SPEAKER_00: Hello, welcome to the meeting.
[2.50s - 5.12s] SPEAKER_01: Thanks for having me.
```

When visual context is enabled, segment metadata may include visual descriptions or face attributes produced by the visual pipeline.
