"""
Face analysis module for identity tracking and emotion detection.

Uses InsightFace for face detection/embeddings and a HuggingFace
vision transformer for emotion classification. No TensorFlow required.
"""

from dataclasses import dataclass, field

import cv2
import numpy as np
import torch
from insightface.app import FaceAnalysis  # type: ignore[import-untyped]
from transformers import AutoImageProcessor, AutoModelForImageClassification


# Emotion adjectives mapping (no emojis)
EMOTION_ADJECTIVES = {
    "angry": "frustrated",
    "disgust": "displeased",
    "fear": "apprehensive",
    "happy": "cheerful",
    "sad": "somber",
    "surprise": "surprised",
    "neutral": "composed",
}


@dataclass
class FaceIdentity:
    """Tracked face identity across video."""

    face_id: str  # "Person A", "Person B", etc.
    embeddings: list[np.ndarray] = field(default_factory=list)

    @property
    def centroid(self) -> np.ndarray:
        """Average embedding for this identity."""
        return np.mean(self.embeddings, axis=0)


@dataclass
class FaceAnalysisResult:
    """Result of face analysis for a single face."""

    face_id: str  # Assigned identity
    emotion: str  # Adjective description
    bbox: tuple[int, int, int, int]  # x, y, w, h


class FaceAnalyzer:
    """
    Analyzes faces for identity and emotion.

    - Uses InsightFace ArcFace for 512-dim embeddings
    - Uses HuggingFace ViT for emotion classification (no TensorFlow)
    """

    # HuggingFace emotion model (small, fast, PyTorch-based)
    EMOTION_MODEL = "dima806/facial_emotions_image_detection"

    def __init__(
        self,
        device: str = "cuda",
        embedding_threshold: float = 0.4,
    ):
        """
        Initialize face analyzer.

        Args:
            device: Device to run on ("cuda" or "cpu").
            embedding_threshold: Cosine distance threshold for same-person matching.
        """
        self.device = device
        self.embedding_threshold = embedding_threshold
        self._face_app: FaceAnalysis | None = None
        self._emotion_model = None
        self._emotion_processor = None
        self._known_identities: list[FaceIdentity] = []
        self._next_person_id = 0

    @property
    def face_app(self) -> FaceAnalysis:
        """Lazy-load InsightFace model with ArcFace."""
        if self._face_app is None:
            providers = (
                ["CUDAExecutionProvider", "CPUExecutionProvider"]
                if self.device == "cuda"
                else ["CPUExecutionProvider"]
            )
            self._face_app = FaceAnalysis(
                name="buffalo_l",  # Full model with ArcFace embeddings
                providers=providers,
            )
            self._face_app.prepare(ctx_id=0 if self.device == "cuda" else -1)
        return self._face_app

    @property
    def emotion_model(self):
        """Lazy-load HuggingFace emotion model."""
        if self._emotion_model is None:
            self._emotion_model = AutoModelForImageClassification.from_pretrained(
                self.EMOTION_MODEL
            ).to(self.device)
            self._emotion_model.eval()
        return self._emotion_model

    @property
    def emotion_processor(self):
        """Lazy-load emotion model processor."""
        if self._emotion_processor is None:
            self._emotion_processor = AutoImageProcessor.from_pretrained(
                self.EMOTION_MODEL
            )
        return self._emotion_processor

    def _get_next_person_name(self) -> str:
        """Generate next person identifier (Person A, Person B, etc.)."""
        name = f"Person {chr(65 + self._next_person_id)}"  # A, B, C, ...
        self._next_person_id += 1
        return name

    def _match_identity(self, embedding: np.ndarray) -> str:
        """
        Match embedding to known identity or create new one.

        Uses cosine distance for matching.
        """
        if not self._known_identities:
            # First face
            identity = FaceIdentity(
                face_id=self._get_next_person_name(),
                embeddings=[embedding],
            )
            self._known_identities.append(identity)
            return identity.face_id

        # Find closest match
        best_match = None
        best_distance = float("inf")

        for identity in self._known_identities:
            # Cosine distance
            distance = 1 - np.dot(embedding, identity.centroid) / (
                np.linalg.norm(embedding) * np.linalg.norm(identity.centroid)
            )
            if distance < best_distance:
                best_distance = distance
                best_match = identity

        if best_distance < self.embedding_threshold and best_match is not None:
            # Match found
            best_match.embeddings.append(embedding)
            return best_match.face_id
        else:
            # New person
            identity = FaceIdentity(
                face_id=self._get_next_person_name(),
                embeddings=[embedding],
            )
            self._known_identities.append(identity)
            return identity.face_id

    def _detect_emotion(self, face_crop: np.ndarray) -> str:
        """
        Detect emotion from face crop using HuggingFace model.

        Args:
            face_crop: BGR face image as numpy array.

        Returns:
            Emotion adjective.
        """
        try:
            # Convert BGR to RGB
            rgb_crop = cv2.cvtColor(face_crop, cv2.COLOR_BGR2RGB)

            # Process for model
            inputs = self.emotion_processor(images=rgb_crop, return_tensors="pt").to(
                self.device
            )

            # Inference
            with torch.no_grad():
                outputs = self.emotion_model(**inputs)
                predicted_idx = outputs.logits.argmax(dim=-1)

            # Get label
            label = self.emotion_model.config.id2label[predicted_idx.item()]
            label_lower = label.lower()

            adjective = EMOTION_ADJECTIVES.get(label_lower, "composed")
            return adjective

        except Exception:
            return "composed"

    def analyze_faces(self, frame: np.ndarray) -> list[FaceAnalysisResult]:
        """
        Analyze all faces in a frame.

        Args:
            frame: BGR image as numpy array.

        Returns:
            List of FaceAnalysisResult for each detected face.
        """
        faces = self.face_app.get(frame)

        results = []

        for face in faces:
            # Get embedding
            embedding = face.embedding
            if embedding is None:
                continue

            # Match to identity
            face_id = self._match_identity(embedding)

            # Get bounding box
            bbox = face.bbox.astype(int)
            x1, y1, x2, y2 = bbox
            w, h = x2 - x1, y2 - y1

            # Crop face for emotion detection
            face_crop = frame[max(0, y1) : y2, max(0, x1) : x2]
            if face_crop.size == 0:
                continue

            # Detect emotion
            emotion = self._detect_emotion(face_crop)

            results.append(
                FaceAnalysisResult(
                    face_id=face_id,
                    emotion=emotion,
                    bbox=(x1, y1, w, h),
                )
            )

        return results
