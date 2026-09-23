from pathlib import Path

import cv2
import numpy as np


def extract_frames_at_timestamps(
    video_path: str | Path,
    timestamps: list[float],
) -> dict[float, np.ndarray]:
    """Extract frames at specific timestamps, keyed by timestamp."""
    cap = cv2.VideoCapture(str(video_path))
    fps = cap.get(cv2.CAP_PROP_FPS) or 30.0
    frames: dict[float, np.ndarray] = {}

    for timestamp in timestamps:
        frame = _extract_frame_at(cap, timestamp, fps)
        if frame is not None:
            frames[timestamp] = frame

    cap.release()
    return frames


def _extract_frame_at(
    cap: cv2.VideoCapture,
    timestamp: float,
    fps: float,
) -> np.ndarray | None:
    """Extract a single frame at the given timestamp."""
    frame_num = int(timestamp * fps)
    cap.set(cv2.CAP_PROP_POS_FRAMES, frame_num)
    ret, frame = cap.read()
    return frame if ret else None
