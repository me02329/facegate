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

// ── YuNet detector ────────────────────────────────────────────────────────────
//
// YuNet (`face_detection_yunet_2023mar.onnx`, MIT, OpenCV Zoo) is the default
// detector since v0.4.0. The struct is still named `ScrfdDetector` until the
// backend abstraction in #16 lands — the public surface is intentionally
// detector-agnostic so the broker and pipeline code do not care which model
// is loaded.
//
// YuNet is anchor-free, fully convolutional, and emits 12 output tensors —
// {cls,obj,bbox,kps} × strides {8, 16, 32}. The post-processing here mirrors
// `opencv/modules/objdetect/src/face_detect.cpp`.

const INPUT_SIZE: u32 = 320;
const CONF_THRESHOLD: f32 = 0.5;
const NMS_IOU_THRESHOLD: f32 = 0.4;

/// YuNet's keypoint head emits landmarks in
/// `[right_eye, left_eye, nose, right_mouth, left_mouth]` order. ArcFace
/// alignment expects `[left_eye, right_eye, nose, left_mouth, right_mouth]`
/// (see `REF_LANDMARKS` in `embedding.rs`). The remap swaps eyes and mouth
/// corners so a face aligned from these landmarks does not come out
/// horizontally mirrored — a subtle bug that would silently tank similarity
/// scores without ever producing a detection-side error.
const YUNET_TO_REFERENCE_LM_ORDER: [usize; 5] = [1, 0, 2, 4, 3];

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

        // YuNet's exported input tensor is named "input".
        let outputs = self
            .session
            .run(ort::inputs!["input" => ort_tensor])
            .map_err(|e| FaceRsError::Detection(format!("inference: {e}")))?;

        let mut dets = parse_yunet_outputs(&outputs, scale_x, scale_y, CONF_THRESHOLD)?;
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

    // YuNet was trained with raw 0-255 BGR pixels, no mean subtraction and
    // no scale factor (OpenCV's default `blobFromImage` call). NCHW layout.
    let sz = INPUT_SIZE as usize;
    let mut tensor = Array4::<f32>::zeros([1, 3, sz, sz]);
    for (y, row) in resized.rows().enumerate() {
        for (x, px) in row.enumerate() {
            tensor[[0, 0, y, x]] = px[2] as f32; // B
            tensor[[0, 1, y, x]] = px[1] as f32; // G
            tensor[[0, 2, y, x]] = px[0] as f32; // R
        }
    }
    Ok((tensor, scale_x, scale_y))
}

// ── Output parsing ────────────────────────────────────────────────────────────
//
// Output tensors per stride S ∈ {8, 16, 32}:
//   cls_S    shape [feat*feat]        — face/no-face score
//   obj_S    shape [feat*feat]        — objectness score
//   bbox_S   shape [feat*feat*4]      — (dx, dy, dw, dh) per cell
//   kps_S    shape [feat*feat*10]     — 5 landmarks × (x, y) per cell
//
// where feat = INPUT_SIZE / S. Decode (matches `face_detect.cpp`):
//   score = sqrt(clamp(cls) * clamp(obj))
//   cx    = (c + dx) * S
//   cy    = (r + dy) * S
//   w     = exp(dw) * S
//   h     = exp(dh) * S
//   x1    = cx - w/2,  y1 = cy - h/2,  x2 = x1 + w,  y2 = y1 + h
//
// (c, r) iterate over the feature map in row-major order. All coordinates
// are in the resized 320×320 input space; we rescale back to original frame
// dimensions before returning.

fn parse_yunet_outputs(
    outputs: &SessionOutputs,
    scale_x: f32,
    scale_y: f32,
    conf_thresh: f32,
) -> Result<Vec<Detection>> {
    let mut detections = Vec::new();

    for &stride in &[8u32, 16, 32] {
        let cls = extract_named(outputs, &format!("cls_{stride}"))?;
        let obj = extract_named(outputs, &format!("obj_{stride}"))?;
        let bboxes = extract_named(outputs, &format!("bbox_{stride}"))?;
        let kps = extract_named(outputs, &format!("kps_{stride}"))?;

        let feat = (INPUT_SIZE / stride) as usize;
        decode_stride(
            &cls,
            &obj,
            &bboxes,
            &kps,
            stride,
            feat,
            scale_x,
            scale_y,
            conf_thresh,
            &mut detections,
        );
    }
    Ok(detections)
}

/// Per-stride YuNet decoder. Extracted from `parse_yunet_outputs` so the
/// arithmetic — bbox formula, landmark order remap, score combination — can
/// be unit-tested with synthetic tensors without standing up an ONNX session.
#[allow(clippy::too_many_arguments)]
fn decode_stride(
    cls: &[f32],
    obj: &[f32],
    bboxes: &[f32],
    kps: &[f32],
    stride: u32,
    feat: usize,
    scale_x: f32,
    scale_y: f32,
    conf_thresh: f32,
    out: &mut Vec<Detection>,
) {
    let cells = feat * feat;
    let usable = cells
        .min(cls.len())
        .min(obj.len())
        .min(bboxes.len() / 4)
        .min(kps.len() / 10);
    if usable < cells {
        tracing::warn!(
            stride,
            cls = cls.len(),
            obj = obj.len(),
            bboxes = bboxes.len(),
            kps = kps.len(),
            "YuNet output tensors are shorter than expected for stride"
        );
    }

    let s = stride as f32;
    for idx in 0..usable {
        let cls_v = cls[idx].clamp(0.0, 1.0);
        let obj_v = obj[idx].clamp(0.0, 1.0);
        let score = (cls_v * obj_v).sqrt();
        if !score.is_finite() || score < conf_thresh {
            continue;
        }

        let c = (idx % feat) as f32;
        let r = (idx / feat) as f32;

        let dx = bboxes[idx * 4];
        let dy = bboxes[idx * 4 + 1];
        let dw = bboxes[idx * 4 + 2];
        let dh = bboxes[idx * 4 + 3];

        let cx = (c + dx) * s;
        let cy = (r + dy) * s;
        let w = dw.exp() * s;
        let h = dh.exp() * s;

        // Reject degenerate boxes before they reach NMS / the embedder.
        if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
            continue;
        }

        let x1 = (cx - w / 2.0) * scale_x;
        let y1 = (cy - h / 2.0) * scale_y;
        let x2 = (cx + w / 2.0) * scale_x;
        let y2 = (cy + h / 2.0) * scale_y;

        // Decode landmarks then remap from YuNet's
        // [right_eye, left_eye, nose, right_mouth, left_mouth] order to
        // ArcFace's [left_eye, right_eye, nose, left_mouth, right_mouth]
        // via `YUNET_TO_REFERENCE_LM_ORDER`.
        let raw: [(f32, f32); 5] = std::array::from_fn(|n| {
            (
                (c + kps[idx * 10 + n * 2]) * s * scale_x,
                (r + kps[idx * 10 + n * 2 + 1]) * s * scale_y,
            )
        });
        let points: [(f32, f32); 5] = std::array::from_fn(|j| raw[YUNET_TO_REFERENCE_LM_ORDER[j]]);

        out.push(Detection {
            bbox: BoundingBox {
                x1,
                y1,
                x2,
                y2,
                confidence: score,
            },
            landmarks: Landmarks { points },
        });
    }
}

fn extract_named(outputs: &SessionOutputs, name: &str) -> Result<Vec<f32>> {
    let Some(output) = outputs.get(name) else {
        let available = outputs.keys().collect::<Vec<_>>().join(", ");
        return Err(FaceRsError::Detection(format!(
            "missing YuNet output '{name}'; available outputs: {available}"
        )));
    };
    let (_shape, data) = output
        .try_extract_tensor::<f32>()
        .map_err(|e| FaceRsError::Detection(format!("extract '{name}': {e}")))?;
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

    /// Build the four YuNet output buffers for a 2×2 feature map with a
    /// single high-confidence detection at cell (1, 0). All other cells
    /// have zero score so they cannot pass the threshold.
    fn single_cell_yunet_tensors(
        active_idx: usize,
        dx: f32,
        dy: f32,
        dw: f32,
        dh: f32,
        landmarks_xy: [(f32, f32); 5],
    ) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let cells = 4;
        let mut cls = vec![0.0f32; cells];
        let mut obj = vec![0.0f32; cells];
        let mut bbox = vec![0.0f32; cells * 4];
        let mut kps = vec![0.0f32; cells * 10];
        cls[active_idx] = 0.95;
        obj[active_idx] = 0.95;
        bbox[active_idx * 4] = dx;
        bbox[active_idx * 4 + 1] = dy;
        bbox[active_idx * 4 + 2] = dw;
        bbox[active_idx * 4 + 3] = dh;
        for (n, (x, y)) in landmarks_xy.iter().enumerate() {
            kps[active_idx * 10 + n * 2] = *x;
            kps[active_idx * 10 + n * 2 + 1] = *y;
        }
        (cls, obj, bbox, kps)
    }

    #[test]
    fn yunet_score_below_threshold_is_dropped() {
        // cls = obj = 0.4 ⇒ sqrt(0.4 * 0.4) = 0.4 < conf_thresh = 0.5
        let mut cls = vec![0.0f32; 4];
        let mut obj = vec![0.0f32; 4];
        let bbox = vec![0.0f32; 16];
        let kps = vec![0.0f32; 40];
        cls[0] = 0.4;
        obj[0] = 0.4;
        let mut out = Vec::new();
        decode_stride(&cls, &obj, &bbox, &kps, 8, 2, 1.0, 1.0, 0.5, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn yunet_decode_recovers_bbox_at_known_cell() {
        // Active cell at (r=1, c=0) on a 2×2 feature map at stride 8.
        // Offsets (dx=0, dy=0, dw=0, dh=0) → w = h = exp(0)*8 = 8, centred
        // on the cell origin. cx = (0 + 0) * 8 = 0, cy = (1 + 0) * 8 = 8.
        // ⇒ bbox = (-4, 4, 4, 12) before frame-scale.
        let (cls, obj, bbox, kps) =
            single_cell_yunet_tensors(2, 0.0, 0.0, 0.0, 0.0, [(0.0, 0.0); 5]);
        let mut out = Vec::new();
        decode_stride(&cls, &obj, &bbox, &kps, 8, 2, 1.0, 1.0, 0.5, &mut out);
        assert_eq!(out.len(), 1);
        let bb = &out[0].bbox;
        assert!((bb.x1 + 4.0).abs() < 1e-4, "x1={}", bb.x1);
        assert!((bb.y1 - 4.0).abs() < 1e-4, "y1={}", bb.y1);
        assert!((bb.x2 - 4.0).abs() < 1e-4, "x2={}", bb.x2);
        assert!((bb.y2 - 12.0).abs() < 1e-4, "y2={}", bb.y2);
    }

    #[test]
    fn yunet_decoder_swaps_left_right_landmarks() {
        // Feed YuNet-order landmarks (right_eye, left_eye, nose,
        // right_mouth, left_mouth) at distinct positions. After decode the
        // `Landmarks` struct must be in ArcFace order
        // (left_eye, right_eye, nose, left_mouth, right_mouth) — i.e. the
        // first and second entries must have swapped, and the fourth and
        // fifth entries must have swapped, relative to what we fed in.
        //
        // Cell (r=0, c=0), stride=8, all delta=0, scale=1.
        // After decode each landmark is (c + kp_x) * s = kp_x * 8.
        let yunet_order: [(f32, f32); 5] = [
            (1.0, 1.1), // YuNet[0] = right_eye
            (2.0, 2.1), // YuNet[1] = left_eye
            (3.0, 3.1), // YuNet[2] = nose
            (4.0, 4.1), // YuNet[3] = right_mouth
            (5.0, 5.1), // YuNet[4] = left_mouth
        ];
        let (cls, obj, bbox, kps) = single_cell_yunet_tensors(0, 0.0, 0.0, 0.0, 0.0, yunet_order);
        let mut out = Vec::new();
        decode_stride(&cls, &obj, &bbox, &kps, 8, 2, 1.0, 1.0, 0.5, &mut out);
        assert_eq!(out.len(), 1);
        let pts = out[0].landmarks.points;
        // Expected ArcFace order with kp * 8:
        //   [0] = left_eye    = YuNet[1] = (16, 16.8)
        //   [1] = right_eye   = YuNet[0] = (8, 8.8)
        //   [2] = nose        = YuNet[2] = (24, 24.8)
        //   [3] = left_mouth  = YuNet[4] = (40, 40.8)
        //   [4] = right_mouth = YuNet[3] = (32, 32.8)
        let expected: [(f32, f32); 5] = [
            (16.0, 16.8),
            (8.0, 8.8),
            (24.0, 24.8),
            (40.0, 40.8),
            (32.0, 32.8),
        ];
        for (i, (got, want)) in pts.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3,
                "lm[{i}] = {got:?}, expected {want:?}"
            );
        }
    }

    #[test]
    fn yunet_decoder_rejects_degenerate_bbox() {
        // dw = -1000 ⇒ exp(-1000) ≈ 0 ⇒ w ≈ 0, should be filtered.
        let (cls, obj, bbox, kps) =
            single_cell_yunet_tensors(0, 0.0, 0.0, -1000.0, 0.0, [(0.0, 0.0); 5]);
        let mut out = Vec::new();
        decode_stride(&cls, &obj, &bbox, &kps, 8, 2, 1.0, 1.0, 0.5, &mut out);
        assert!(out.is_empty());
    }
}
