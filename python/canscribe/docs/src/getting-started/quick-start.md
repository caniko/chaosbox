# Quick Start

Enter the development shell:

```sh
nix develop
```

Sync dependencies for your platform:

```sh
uv sync --extra nvidia
```

Set the Hugging Face token used by pyannote:

```sh
export HF_TOKEN="hf_your_token_here"
```

Run a transcription:

```sh
canscribe ~/Videos/meeting.mp4
```

The output is written as `transcript-meeting.txt` next to the input media file.

## Speaker Counts

If the number of speakers is known, pass it explicitly:

```sh
canscribe --speakers 5 ~/Videos/meeting.mp4
```

If only a range is known:

```sh
canscribe --min-speakers 3 --max-speakers 6 ~/Videos/meeting.mp4
```

## Compatibility Backend

Use Moonshine when you need the compatibility backend:

```sh
canscribe --asr moonshine --model UsefulSensors/moonshine-tiny ~/Videos/meeting.mp4
```
