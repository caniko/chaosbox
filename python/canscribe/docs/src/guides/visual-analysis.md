# Visual Analysis

Visual analysis is enabled with `--visual` and is only valid for video inputs.

```sh
canscribe --visual ~/Videos/meeting.mp4
```

Frames with detected faces use face analysis. Frames without faces use scene description. Face embeddings use InsightFace `buffalo_l`, emotion detection uses the HuggingFace `dima806/facial_emotions_image_detection` model, and scene descriptions use `Qwen/Qwen3-VL-4B-Instruct`.

Visual mode adds model load time and GPU memory pressure. Use `canscribe check <video-file>` first when validating a new machine or media source.
