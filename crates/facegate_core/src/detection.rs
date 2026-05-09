use std::path::Path;

use image::imageops::FilterType;
use ndarray::Array4;
use ort::session::{builder::GraphOptimizationLevel, Session, SessionOutputs};
use ort::value::Tensor;

use crate::camera::Frame;
use crate::error::{FaceRsError, Result};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub confidence: f32,
}

impl BoundingBox {
    pub fn width(&self) -> f32 {
        self.x2 - self.x1
    }
    pub fn height(&self) -> f32 {
        self.y2 - self.y1
    }
    pub fn area(&self) -> f32 {
        self.width().max(0.0) * self.height().max(0.0)
    }
}

/// Five facial landmarks: left eye, right eye, nose, left mouth, right mouth.
#[derive(Debug, Clone, Copy)]
pub struct Landmarks {
    pub points: [(f32, f32); 5],
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub bbox: BoundingBox,
    pub landmarks: Landmarks,
}

// ── SCRFD detector ────────────────────────────────────────────────────────────

const INPUT_SIZE: u32 = 640;
const CONF_THRESHOLD: f32 = 0.5;
const NMS_IOU_THRESHOLD: f32 = 0.4;

pub struct ScrfdDetector {
    session: Session,
}

impl ScrfdDetector {
    pub fn load(model_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| FaceRsError::Detection(format!("session builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| FaceRsError::Detection(format!("opt level: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| FaceRsError::Detection(format!("load {}: {e}", model_path.display())))?;

        Ok(ScrfdDetector { session })
    }

    pub fn detect(&mut self, frame: &Frame, min_size: u32) -> Result<Vec<Detection>> {
        let (tensor, scale_x, scale_y) = preprocess(frame)?;

        let ort_tensor = Tensor::<f32>::from_array(tensor)
            .map_err(|e| FaceRsError::Detection(format!("tensor creation: {e}")))?;

        let outputs = self
            .session
            .run(ort::inputs!["input.1" => ort_tensor])
            .map_err(|e| FaceRsError::Detection(format!("inference: {e}")))?;

        let mut dets = parse_scrfd_outputs(&outputs, scale_x, scale_y, CONF_THRESHOLD)?;
        dets = nms(dets, NMS_IOU_THRESHOLD);
        dets.retain(|d| d.bbox.width() >= min_size as f32 && d.bbox.height() >= min_size as f32);
        Ok(dets)
    }
}

// ── Preprocessing ─────────────────────────────────────────────────────────────

fn preprocess(frame: &Frame) -> Result<(Array4<f32>, f32, f32)> {
    let scale_x = frame.width as f32 / INPUT_SIZE as f32;
    let scale_y = frame.height as f32 / INPUT_SIZE as f32;

    // `Triangle` = bilinear in image 0.25
    let resized = image::imageops::resize(
        &frame.to_image(),
        INPUT_SIZE,
        INPUT_SIZE,
        FilterType::Triangle,
    );

    // NCHW, BGR, normalised: (pixel - 127.5) / 128
    let sz = INPUT_SIZE as usize;
    let mut tensor = Array4::<f32>::zeros([1, 3, sz, sz]);
    for (y, row) in resized.rows().enumerate() {
        for (x, px) in row.enumerate() {
            tensor[[0, 0, y, x]] = (px[2] as f32 - 127.5) / 128.0; // B
            tensor[[0, 1, y, x]] = (px[1] as f32 - 127.5) / 128.0; // G
            tensor[[0, 2, y, x]] = (px[0] as f32 - 127.5) / 128.0; // R
        }
    }
    Ok((tensor, scale_x, scale_y))
}

// ── Output parsing ────────────────────────────────────────────────────────────
//
// SCRFD exports commonly use either descriptive output names:
//   score_{stride}, bbox_{stride}, kps_{stride}
// or numeric graph node names in this order:
//   scores for 8/16/32, boxes for 8/16/32, landmarks for 8/16/32.

fn parse_scrfd_outputs(
    outputs: &SessionOutputs,
    scale_x: f32,
    scale_y: f32,
    conf_thresh: f32,
) -> Result<Vec<Detection>> {
    let mut detections = Vec::new();

    for (level, &stride) in [8u32, 16, 32].iter().enumerate() {
        let scores = extract_flat(outputs, &format!("score_{stride}"), level)?;
        let bboxes = extract_flat(outputs, &format!("bbox_{stride}"), 3 + level)?;
        let kps = extract_flat(outputs, &format!("kps_{stride}"), 6 + level)?;

        let feat = (INPUT_SIZE / stride) as usize;
        let n = feat * feat * 2; // anchors per stride

        let usable = n
            .min(scores.len())
            .min(bboxes.len() / 4)
            .min(kps.len() / 10);
        if usable < n.min(scores.len()) {
            tracing::warn!(
                stride,
                scores = scores.len(),
                bboxes = bboxes.len(),
                kps = kps.len(),
                "SCRFD output tensors are shorter than expected"
            );
        }

        for i in 0..usable {
            let conf = scores[i];
            if conf < conf_thresh {
                continue;
            }

            let ax = ((i / 2) % feat) as f32 * stride as f32;
            let ay = ((i / 2) / feat) as f32 * stride as f32;
            let s = stride as f32;

            let x1 = (ax - bboxes[i * 4] * s) * scale_x;
            let y1 = (ay - bboxes[i * 4 + 1] * s) * scale_y;
            let x2 = (ax + bboxes[i * 4 + 2] * s) * scale_x;
            let y2 = (ay + bboxes[i * 4 + 3] * s) * scale_y;

            let mut points = [(0.0f32, 0.0f32); 5];
            for j in 0..5 {
                points[j] = (
                    (ax + kps[i * 10 + j * 2] * s) * scale_x,
                    (ay + kps[i * 10 + j * 2 + 1] * s) * scale_y,
                );
            }

            detections.push(Detection {
                bbox: BoundingBox {
                    x1,
                    y1,
                    x2,
                    y2,
                    confidence: conf,
                },
                landmarks: Landmarks { points },
            });
        }
    }
    Ok(detections)
}

fn extract_flat(outputs: &SessionOutputs, name: &str, fallback_index: usize) -> Result<Vec<f32>> {
    if let Some(output) = outputs.get(name) {
        let (_shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| FaceRsError::Detection(format!("extract '{name}': {e}")))?;
        return Ok(data.to_vec());
    }

    let Some((actual_name, output)) = outputs.iter().nth(fallback_index) else {
        let available = outputs.keys().collect::<Vec<_>>().join(", ");
        return Err(FaceRsError::Detection(format!(
            "missing output '{name}' and fallback output #{fallback_index}; available outputs: {available}"
        )));
    };
    let (_shape, data) = output
        .try_extract_tensor::<f32>()
        .map_err(|e| FaceRsError::Detection(format!("extract '{actual_name}': {e}")))?;
    Ok(data.to_vec())
}

// ── NMS ───────────────────────────────────────────────────────────────────────

fn nms(mut dets: Vec<Detection>, iou_thresh: f32) -> Vec<Detection> {
    dets.sort_by(|a, b| b.bbox.confidence.total_cmp(&a.bbox.confidence));
    let mut keep = Vec::new();
    let mut sup = vec![false; dets.len()];
    for i in 0..dets.len() {
        if sup[i] {
            continue;
        }
        keep.push(dets[i].clone());
        for j in (i + 1)..dets.len() {
            if iou(&dets[i].bbox, &dets[j].bbox) > iou_thresh {
                sup[j] = true;
            }
        }
    }
    keep
}

fn iou(a: &BoundingBox, b: &BoundingBox) -> f32 {
    let inter =
        ((a.x2.min(b.x2) - a.x1.max(b.x1)).max(0.0)) * ((a.y2.min(b.y2) - a.y1.max(b.y1)).max(0.0));
    let union = a.area() + b.area() - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

// ── Stub ──────────────────────────────────────────────────────────────────────

pub struct StubDetector;

impl StubDetector {
    pub fn load(_: &Path) -> Result<Self> {
        Ok(StubDetector)
    }

    pub fn detect(&mut self, _: &Frame, _: u32) -> Result<Vec<Detection>> {
        Err(FaceRsError::Detection(
            "stub detector: no model loaded".to_string(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(x1: f32, y1: f32, x2: f32, y2: f32) -> BoundingBox {
        BoundingBox {
            x1,
            y1,
            x2,
            y2,
            confidence: 1.0,
        }
    }

    #[test]
    fn iou_identical() {
        assert!((iou(&bb(0., 0., 10., 10.), &bb(0., 0., 10., 10.)) - 1.0).abs() < 1e-5);
    }
    #[test]
    fn iou_no_overlap() {
        assert_eq!(iou(&bb(0., 0., 5., 5.), &bb(10., 10., 15., 15.)), 0.0);
    }

    #[test]
    fn nms_dedup() {
        let make = |c: f32| Detection {
            bbox: BoundingBox {
                x1: 0.,
                y1: 0.,
                x2: 10.,
                y2: 10.,
                confidence: c,
            },
            landmarks: Landmarks {
                points: [(0., 0.); 5],
            },
        };
        assert_eq!(nms(vec![make(0.9), make(0.8)], 0.4).len(), 1);
    }
}
