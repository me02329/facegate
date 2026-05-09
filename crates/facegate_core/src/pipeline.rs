use std::time::{Duration, Instant};

use crate::camera::V4lCamera;
use crate::config::Config;
use crate::detection::ScrfdDetector;
use crate::embedding::ArcFaceEmbedder;
use crate::error::{FaceRsError, Result};
use crate::matching::Embedding;

/// Full face authentication pipeline: camera → detector → embedder.
pub struct FacePipeline {
    camera: V4lCamera,
    detector: ScrfdDetector,
    embedder: ArcFaceEmbedder,
}

impl FacePipeline {
    /// Open the camera and load both ONNX models.
    pub fn new(config: &Config) -> Result<Self> {
        tracing::debug!(device = %config.camera.device, "opening camera");
        let mut camera = V4lCamera::open(
            &config.camera.device,
            config.camera.width,
            config.camera.height,
            config.camera.fps,
            config.camera.timeout_ms,
        )?;
        camera.warmup(config.camera.warmup_frames);

        tracing::debug!(path = %config.models.detector.display(), "loading detector");
        let detector = ScrfdDetector::load(&config.models.detector)?;

        tracing::debug!(path = %config.models.embedder.display(), "loading embedder");
        let embedder = ArcFaceEmbedder::load(&config.models.embedder)?;

        Ok(FacePipeline {
            camera,
            detector,
            embedder,
        })
    }

    /// Capture frames until a face is found or the timeout elapses.
    /// Returns the L2-normalised embedding of the largest detected face.
    pub fn capture_embedding(&mut self, config: &Config) -> Result<Embedding> {
        let timeout = Duration::from_millis(config.camera.timeout_ms);
        let deadline = Instant::now() + timeout;

        loop {
            if Instant::now() >= deadline {
                return Err(FaceRsError::Timeout);
            }

            let frame = self.camera.capture_frame()?;
            let dets = self
                .detector
                .detect(&frame, config.recognition.min_face_size)?;

            // Pick the largest face by bounding-box area
            if let Some(det) = dets
                .into_iter()
                .max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area()))
            {
                tracing::debug!(conf = det.bbox.confidence, "face detected");
                return self.embedder.extract(&frame, &det);
            }
        }
    }
}
