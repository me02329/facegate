use std::path::Path;

use image::imageops::FilterType;
use image::{ImageBuffer, Rgb};
use ndarray::Array4;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;

use crate::camera::Frame;
use crate::detection::{Detection, Landmarks};
use crate::error::{FaceRsError, Result};
use crate::matching::Embedding;

// ── ArcFace input spec ────────────────────────────────────────────────────────

const FACE_SIZE: u32 = 112;

// Reference landmark positions for 112×112 alignment (ArcFace standard)
const REF_LANDMARKS: [(f32, f32); 5] = [
    (38.29, 51.70), // left eye
    (73.53, 51.50), // right eye
    (56.02, 71.74), // nose tip
    (41.55, 92.37), // left mouth
    (70.72, 92.20), // right mouth
];

// ── ArcFace embedder ──────────────────────────────────────────────────────────

pub struct ArcFaceEmbedder {
    session: Session,
}

impl ArcFaceEmbedder {
    pub fn load(model_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| FaceRsError::Embedding(format!("session builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| FaceRsError::Embedding(format!("opt level: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| FaceRsError::Embedding(format!("load {}: {e}", model_path.display())))?;

        Ok(ArcFaceEmbedder { session })
    }

    /// Align the face from `frame` using the detected `landmarks`, run
    /// ArcFace inference, and return the L2-normalised 512-d embedding.
    pub fn extract(&mut self, frame: &Frame, detection: &Detection) -> Result<Embedding> {
        let aligned = align_face(&frame.to_image(), &detection.landmarks)?;
        let tensor = to_tensor(&aligned)?;

        let ort_tensor = Tensor::<f32>::from_array(tensor)
            .map_err(|e| FaceRsError::Embedding(format!("tensor: {e}")))?;

        // Capture the output name before borrowing session mutably for run()
        let output_name = self.session.outputs()[0].name().to_owned();

        let outputs = self
            .session
            .run(ort::inputs!["input.1" => ort_tensor])
            .map_err(|e| FaceRsError::Embedding(format!("inference: {e}")))?;

        let (_shape, raw) = outputs[output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|e| FaceRsError::Embedding(format!("extract: {e}")))?;

        Ok(l2_normalize(raw))
    }
}

// ── Face alignment ────────────────────────────────────────────────────────────
//
// We use a simple similarity transform (scale + rotation + translation) to map
// the detected landmarks onto the ArcFace reference positions.  A full affine
// warp is not needed for a similarity transform — we estimate it via the
// least-squares formula on the 5-point correspondence.

fn align_face(
    img: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    lm: &Landmarks,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>> {
    let (a, b, c, d) = similarity_transform(&lm.points, &REF_LANDMARKS);
    Ok(warp_affine(img, a, b, c, d, FACE_SIZE, FACE_SIZE))
}

/// Returns (a, b, tx, ty) for the similarity warp:
///   x' = a*x - b*y + tx
///   y' = b*x + a*y + ty
fn similarity_transform(src: &[(f32, f32); 5], dst: &[(f32, f32); 5]) -> (f32, f32, f32, f32) {
    // Solve: min ||[a -b; b a] * src_i + t - dst_i||²
    // Closed-form solution:
    let n = src.len() as f32;
    let (mut sx, mut sy, mut dx, mut dy) = (0f32, 0f32, 0f32, 0f32);
    let (mut sxx_syy, mut sxy_syx) = (0f32, 0f32);

    for (s, d) in src.iter().zip(dst.iter()) {
        sx += s.0;
        sy += s.1;
        dx += d.0;
        dy += d.1;
        sxx_syy += s.0 * s.0 + s.1 * s.1;
        sxy_syx += s.0 * d.0 + s.1 * d.1;
    }

    let denom = n * sxx_syy - sx * sx - sy * sy;
    if denom.abs() < 1e-8 {
        return (1.0, 0.0, 0.0, 0.0);
    }

    let a = (n * sxy_syx - sx * dx - sy * dy) / denom;

    let mut b_num = 0f32;
    for (s, d) in src.iter().zip(dst.iter()) {
        b_num += s.0 * d.1 - s.1 * d.0;
    }
    let b = (n * b_num - sx * dy + sy * dx) / denom;

    let tx = (dx - a * sx + b * sy) / n;
    let ty = (dy - b * sx - a * sy) / n;

    (a, b, tx, ty)
}

/// Warp image with similarity transform using nearest-neighbour sampling.
fn warp_affine(
    src: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    a: f32,
    b: f32,
    tx: f32,
    ty: f32,
    out_w: u32,
    out_h: u32,
) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    // Inverse transform: given output (x', y'), compute source (x, y)
    let det = a * a + b * b;
    let (ia, ib) = if det > 1e-8 {
        (a / det, -b / det)
    } else {
        (1.0, 0.0)
    };

    let sw = src.width() as i32;
    let sh = src.height() as i32;

    ImageBuffer::from_fn(out_w, out_h, |xp, yp| {
        let xp = xp as f32;
        let yp = yp as f32;
        let sx = ia * (xp - tx) + ib * (yp - ty);
        let sy = -ib * (xp - tx) + ia * (yp - ty);
        let xi = sx.round() as i32;
        let yi = sy.round() as i32;
        if xi >= 0 && xi < sw && yi >= 0 && yi < sh {
            *src.get_pixel(xi as u32, yi as u32)
        } else {
            image::Rgb([0u8, 0, 0])
        }
    })
}

// ── Preprocessing for ArcFace ─────────────────────────────────────────────────

fn to_tensor(img: &ImageBuffer<Rgb<u8>, Vec<u8>>) -> Result<Array4<f32>> {
    // Resize to 112×112 just in case alignment produced a different size
    let img = image::imageops::resize(img, FACE_SIZE, FACE_SIZE, FilterType::Triangle);
    let sz = FACE_SIZE as usize;

    // NCHW, RGB, normalised: (pixel / 255 - 0.5) / 0.5  →  [-1, 1]
    let mut tensor = Array4::<f32>::zeros([1, 3, sz, sz]);
    for (y, row) in img.rows().enumerate() {
        for (x, px) in row.enumerate() {
            tensor[[0, 0, y, x]] = (px[0] as f32 / 255.0 - 0.5) / 0.5; // R
            tensor[[0, 1, y, x]] = (px[1] as f32 / 255.0 - 0.5) / 0.5; // G
            tensor[[0, 2, y, x]] = (px[2] as f32 / 255.0 - 0.5) / 0.5; // B
        }
    }
    Ok(tensor)
}

// ── L2 normalisation ─────────────────────────────────────────────────────────

fn l2_normalize(v: &[f32]) -> Embedding {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-8 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

// ── Stub ──────────────────────────────────────────────────────────────────────

pub struct StubEmbedder;

impl StubEmbedder {
    pub fn load(_: &Path) -> Result<Self> {
        Ok(StubEmbedder)
    }

    pub fn extract(&mut self, _: &Frame, _: &Detection) -> Result<Embedding> {
        Err(FaceRsError::Embedding(
            "stub embedder: no model loaded".to_string(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_normalize_unit_vector() {
        let v = vec![3.0f32, 4.0];
        let n = l2_normalize(&v);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!((n[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_safe() {
        let v = vec![0.0f32, 0.0, 0.0];
        let n = l2_normalize(&v);
        assert_eq!(n, v);
    }

    #[test]
    fn similarity_transform_identity_like() {
        // Source and destination are the same → should be close to identity (a≈1, b≈0)
        let pts: [(f32, f32); 5] = [(10., 10.), (20., 10.), (15., 17.), (12., 24.), (18., 24.)];
        let (a, b, tx, ty) = similarity_transform(&pts, &pts);
        assert!((a - 1.0).abs() < 1e-4, "a={a}");
        assert!(b.abs() < 1e-4, "b={b}");
        assert!(tx.abs() < 1e-3, "tx={tx}");
        assert!(ty.abs() < 1e-3, "ty={ty}");
    }
}
