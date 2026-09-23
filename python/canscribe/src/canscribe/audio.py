import subprocess
import tempfile
from pathlib import Path

from .config import VIDEO_EXTENSIONS


def extract_audio_from_video(video_path: str) -> str:
    """Extract audio from video file to a temporary WAV file."""
    temp_audio = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
    temp_audio.close()

    subprocess.run(
        [
            "ffmpeg",
            "-y",
            "-hwaccel",
            "auto",  # Enable hardware acceleration (NVDEC/VAAPI/etc.)
            "-i",
            video_path,
            "-vn",
            "-acodec",
            "pcm_s16le",
            "-ar",
            "16000",
            "-ac",
            "1",
            temp_audio.name,
        ],
        check=True,
        capture_output=True,
    )
    return temp_audio.name


def get_audio_path(file_path: str) -> tuple[str, str | None]:
    """
    Get the audio path from a file.

    Returns:
        Tuple of (audio_path, temp_file_path or None if no temp file was created)
    """
    if Path(file_path).suffix.lower() in VIDEO_EXTENSIONS:
        print("\U0001f3ac Extracting audio from video...")
        temp_path = extract_audio_from_video(file_path)
        return temp_path, temp_path
    return file_path, None
