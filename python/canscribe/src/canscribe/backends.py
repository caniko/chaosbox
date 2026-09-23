from __future__ import annotations

import os
import wave
from typing import Any, Protocol, cast

import numpy as np
import torch
import torchaudio  # type: ignore[import-untyped]

from .config import (
    ASR_MAX_NEW_TOKENS,
    MOONSHINE_MODEL,
    PARAKEET_MODEL,
    PYANNOTE_MODEL,
    get_attention_config,
)
from .formatting import clean_repetitive_text
from .types import (
    AsrBackendName,
    AsrResult,
    BackendSetupError,
    DiarizationSegment,
)


def _install_rocm_rnn_dropout_guard() -> None:
    """Force ``dropout=0`` on every RNN layer built while ROCm is active.

    PyTorch's ROCm wheels ship a MIOpen that JIT-compiles its RNN dropout
    kernel against rocrand headers absent from the wheel, so any multi-layer
    LSTM/GRU/RNN with ``dropout > 0`` aborts with ``miopenStatusUnknownError``
    on first forward (pytorch/pytorch#160141, closed unfixed). pyannote builds
    its segmentation model lazily on first inference, so we patch
    ``RNNBase.__init__`` to clear the dropout hyperparameter as the layers are
    constructed. Inter-layer dropout is an identity operation at inference, so
    this is numerically exact and keeps the recurrent layers on the GPU.
    No-op off ROCm (``torch.version.hip`` unset) and installed only once.
    """
    if not getattr(torch.version, "hip", None):
        return
    rnn_base = torch.nn.RNNBase
    if getattr(rnn_base, "_canscribe_dropout_guard", False):
        return
    original_init = rnn_base.__init__

    def guarded_init(self: torch.nn.RNNBase, *args: Any, **kwargs: Any) -> None:
        original_init(self, *args, **kwargs)
        if self.dropout:
            self.dropout = 0.0

    rnn_base.__init__ = guarded_init  # type: ignore[method-assign]
    rnn_base._canscribe_dropout_guard = True  # type: ignore[attr-defined]


class AsrBackend(Protocol):
    name: str
    model_name: str

    def transcribe_chunk(
        self, audio_chunk: torch.Tensor, sample_rate: int
    ) -> AsrResult:
        """Transcribe one mono audio chunk."""


class DiarizationBackend(Protocol):
    name: str
    model_name: str

    def diarize(
        self,
        audio_path: str,
        *,
        speaker_count: int | None = None,
        min_speakers: int | None = None,
        max_speakers: int | None = None,
    ) -> tuple[list[DiarizationSegment], tuple[str, ...]]:
        """Return diarization segments and non-fatal warnings."""


class ParakeetAsrBackend:
    """Transformers ASR backend for NVIDIA Parakeet TDT v3.

    The backend intentionally uses Transformers/PyTorch only. It does not depend on
    NeMo, CUDA-specific runtime packages, or NVIDIA-only inference servers, so ROCm
    remains a supported PyTorch execution path.
    """

    name = "parakeet"

    def __init__(self, model_name: str = PARAKEET_MODEL, device: str = "cpu") -> None:
        self.model_name = model_name
        self.device = device
        self._pipeline: Any | None = None

    @property
    def pipeline(self) -> Any:
        if self._pipeline is None:
            try:
                from transformers import pipeline

                pipeline_device: int | str
                if self.device == "cuda":
                    pipeline_device = 0
                elif self.device == "mps":
                    pipeline_device = "mps"
                else:
                    pipeline_device = -1

                self._pipeline = pipeline(
                    task="automatic-speech-recognition",
                    model=self.model_name,
                    device=pipeline_device,
                )
            except Exception as exc:
                raise BackendSetupError(
                    "Failed to initialize ASR backend 'parakeet' with model "
                    f"'{self.model_name}' on device '{self.device}'. This backend must "
                    "load through Transformers/PyTorch; install a compatible PyTorch "
                    "extra for your platform such as `uv sync --extra amd`, "
                    "`uv sync --extra nvidia`, or `uv sync --extra cpu`."
                ) from exc
        return self._pipeline

    def transcribe_chunk(
        self, audio_chunk: torch.Tensor, sample_rate: int
    ) -> AsrResult:
        audio_array = audio_chunk.detach().to("cpu").float().numpy()
        with torch.inference_mode():
            output = self.pipeline(
                {"array": audio_array, "sampling_rate": sample_rate},
                max_new_tokens=ASR_MAX_NEW_TOKENS,
            )

        if isinstance(output, str):
            return AsrResult(text=clean_repetitive_text(output))
        if not isinstance(output, dict):
            raise BackendSetupError(
                f"ASR backend 'parakeet' returned unsupported output type: {type(output).__name__}"
            )

        text = clean_repetitive_text(str(output.get("text", "")))
        return AsrResult(text=text)


class MoonshineAsrBackend:
    """Compatibility ASR backend for existing Moonshine workflows."""

    name = "moonshine"

    def __init__(self, model_name: str = MOONSHINE_MODEL, device: str = "cpu") -> None:
        self.model_name = model_name
        self.device = device
        self._processor: Any | None = None
        self._model: Any | None = None

    @property
    def processor(self) -> Any:
        if self._processor is None:
            try:
                from transformers import AutoProcessor

                self._processor = AutoProcessor.from_pretrained(self.model_name)
            except Exception as exc:
                raise BackendSetupError(
                    f"Failed to load Moonshine processor for '{self.model_name}'."
                ) from exc
        return self._processor

    @property
    def model(self) -> Any:
        if self._model is None:
            try:
                from transformers import MoonshineForConditionalGeneration

                attention, dtype = get_attention_config(self.device)
                self._model = MoonshineForConditionalGeneration.from_pretrained(
                    self.model_name,
                    attn_implementation=attention,
                    dtype=dtype,
                ).to(self.device)  # type: ignore[arg-type]  # transformers 5.x from_pretrained wrapper stub
                self._model.eval()
            except Exception as exc:
                raise BackendSetupError(
                    "Failed to initialize ASR backend 'moonshine' with model "
                    f"'{self.model_name}' on device '{self.device}'."
                ) from exc
        return self._model

    def transcribe_chunk(
        self, audio_chunk: torch.Tensor, sample_rate: int
    ) -> AsrResult:
        inputs = self.processor(
            audio_chunk.detach().to("cpu"),
            sampling_rate=sample_rate,
            return_tensors="pt",
        ).to(self.device)
        with torch.inference_mode():
            generated_ids = self.model.generate(
                **inputs, max_new_tokens=ASR_MAX_NEW_TOKENS
            )
        text = self.processor.batch_decode(generated_ids, skip_special_tokens=True)[0]
        return AsrResult(text=clean_repetitive_text(text.strip()))


class PyannoteCommunityBackend:
    """Local pyannote Community-1 diarization backend."""

    name = "pyannote-community"

    def __init__(self, model_name: str = PYANNOTE_MODEL, device: str = "cpu") -> None:
        self.model_name = model_name
        self.device = device
        self._pipeline: Any | None = None

    @property
    def pipeline(self) -> Any:
        if self._pipeline is None:
            token = os.environ.get("HF_TOKEN")
            if not token:
                raise BackendSetupError(
                    "Missing required HF_TOKEN for local pyannote Community-1 diarization. "
                    "Accept access to pyannote/speaker-diarization-community-1 on "
                    "Hugging Face, then run `export HF_TOKEN=hf_...` before transcribing."
                )

            try:
                from pyannote.audio import Pipeline
                from pyannote.audio.core.task import Problem, Resolution, Specifications

                torch.serialization.add_safe_globals(
                    [Problem, Resolution, Specifications]
                )
                _install_rocm_rnn_dropout_guard()
                loaded = Pipeline.from_pretrained(self.model_name, token=token)
                if loaded is None:
                    raise BackendSetupError(
                        f"pyannote returned no pipeline for '{self.model_name}'."
                    )
                self._pipeline = loaded.to(torch.device(self.device))
            except BackendSetupError:
                raise
            except Exception as exc:
                raise BackendSetupError(
                    "Failed to initialize local diarization backend "
                    f"'{self.model_name}' on device '{self.device}'."
                ) from exc
        return self._pipeline

    def diarize(
        self,
        audio_path: str,
        *,
        speaker_count: int | None = None,
        min_speakers: int | None = None,
        max_speakers: int | None = None,
    ) -> tuple[list[DiarizationSegment], tuple[str, ...]]:
        warnings: list[str] = []
        try:
            from pyannote.audio.pipelines.utils.hook import ProgressHook

            waveform, sample_rate = _load_audio_tensor(audio_path)
            speaker_options = _speaker_count_options(
                speaker_count=speaker_count,
                min_speakers=min_speakers,
                max_speakers=max_speakers,
            )

            with ProgressHook() as hook:
                diarization_result = self.pipeline(
                    {"waveform": waveform, "sample_rate": sample_rate},
                    hook=hook,
                    **speaker_options,
                )
        except Exception as exc:
            raise BackendSetupError(
                f"Local diarization failed for '{audio_path}' using '{self.model_name}'."
            ) from exc

        try:
            timeline = diarization_result.exclusive_speaker_diarization
        except AttributeError:
            warnings.append(
                "Diarization result did not expose exclusive_speaker_diarization; "
                "using the standard pyannote timeline."
            )
            timeline = diarization_result

        segments = [
            DiarizationSegment(
                start=float(segment.start),
                end=float(segment.end),
                speaker=str(speaker),
            )
            for segment, _, speaker in timeline.itertracks(yield_label=True)
        ]
        return segments, tuple(warnings)


def _speaker_count_options(
    *,
    speaker_count: int | None,
    min_speakers: int | None,
    max_speakers: int | None,
) -> dict[str, int]:
    options: dict[str, int] = {}
    if speaker_count is not None:
        options["num_speakers"] = speaker_count
    if min_speakers is not None:
        options["min_speakers"] = min_speakers
    if max_speakers is not None:
        options["max_speakers"] = max_speakers
    return options


def create_asr_backend(
    backend: AsrBackendName | str,
    *,
    model_name: str | None = None,
    device: str,
) -> AsrBackend:
    if backend == "parakeet":
        return ParakeetAsrBackend(model_name or PARAKEET_MODEL, device=device)
    if backend == "moonshine":
        return MoonshineAsrBackend(model_name or MOONSHINE_MODEL, device=device)
    raise BackendSetupError(
        f"Unknown ASR backend '{backend}'. Expected one of: parakeet, moonshine."
    )


def load_mono_audio(
    audio_path: str, target_sample_rate: int = 16000
) -> tuple[torch.Tensor, int]:
    waveform, sample_rate = _load_audio_tensor(audio_path)
    if sample_rate != target_sample_rate:
        if _can_use_torchaudio_resample():
            resampler = torchaudio.transforms.Resample(
                orig_freq=sample_rate,
                new_freq=target_sample_rate,
            )
            waveform = resampler(waveform)
        else:
            waveform = torch.nn.functional.interpolate(
                waveform.unsqueeze(0),
                size=max(
                    1, int(round(waveform.shape[-1] * target_sample_rate / sample_rate))
                ),
                mode="linear",
                align_corners=False,
            ).squeeze(0)
        sample_rate = target_sample_rate
    return cast(torch.Tensor, waveform[0]), sample_rate


def _load_audio_tensor(audio_path: str) -> tuple[torch.Tensor, int]:
    try:
        waveform, sample_rate = torchaudio.load(audio_path)
    except Exception:
        waveform, sample_rate = _load_pcm_wav(audio_path)

    if waveform.ndim != 2:
        raise BackendSetupError(
            f"Expected a 2D waveform tensor for '{audio_path}', got shape {tuple(waveform.shape)}."
        )
    if waveform.shape[0] > 1:
        waveform = waveform.mean(dim=0, keepdim=True)
    return waveform, sample_rate


def _load_pcm_wav(audio_path: str) -> tuple[torch.Tensor, int]:
    try:
        with wave.open(audio_path, "rb") as handle:
            sample_rate = handle.getframerate()
            channels = handle.getnchannels()
            sample_width = handle.getsampwidth()
            frames = handle.readframes(handle.getnframes())
    except wave.Error as exc:
        raise BackendSetupError(
            f"Fallback WAV loader could not read '{audio_path}'."
        ) from exc

    dtype_map = {
        1: np.uint8,
        2: np.int16,
        4: np.int32,
    }
    dtype = dtype_map.get(sample_width)
    if dtype is None:
        raise BackendSetupError(
            f"Unsupported WAV sample width {sample_width} bytes for '{audio_path}'."
        )

    samples = np.frombuffer(frames, dtype=dtype)
    if channels > 1:
        waveform = torch.from_numpy(
            samples.reshape(-1, channels).T.astype(np.float32, copy=False)
        )
    else:
        waveform = torch.from_numpy(
            samples.reshape(1, -1).astype(np.float32, copy=False)
        )
    if sample_width == 1:
        waveform = (waveform - 128.0) / 128.0
    elif sample_width == 2:
        waveform = waveform / 32768.0
    else:
        waveform = waveform / 2147483648.0
    return waveform, sample_rate


def _can_use_torchaudio_resample() -> bool:
    try:
        torchaudio.transforms.Resample
    except Exception:
        return False
    return True
