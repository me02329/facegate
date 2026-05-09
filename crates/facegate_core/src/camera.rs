use image::{ImageBuffer, Rgb};
use v4l::buffer::Type;
use v4l::io::mmap::Stream as MmapStream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

use crate::error::{FaceRsError, Result};
use std::ptr::NonNull;

/// RGB24 frame ready for ML preprocessing.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Raw RGB bytes, row-major, 3 bytes per pixel.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    /// Decode into an `image` RgbImage for resizing / cropping.
    pub fn to_image(&self) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
        ImageBuffer::from_raw(self.width, self.height, self.data.clone())
            .unwrap_or_else(|| ImageBuffer::new(self.width, self.height))
    }
}

// ── V4L2 camera ───────────────────────────────────────────────────────────────

pub struct V4lCamera {
    device_path: String,
    stream: Option<MmapStream<'static>>,
    device: NonNull<Device>,
    width: u32,
    height: u32,
    format: CaptureFormat,
    fourcc: FourCC,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureFormat {
    Yuyv,
    Mjpeg,
    Grey,
}

impl V4lCamera {
    pub fn open(device: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        let dev = Device::with_path(device)
            .map_err(|e| FaceRsError::Camera(format!("cannot open {device}: {e}")))?;

        // Negotiate format: prefer MJPEG/YUYV, but keep GREY for IR cameras.
        let mut fmt = dev
            .format()
            .map_err(|e| FaceRsError::Camera(format!("cannot query format: {e}")))?;

        fmt.width = width;
        fmt.height = height;

        let negotiated = negotiate_format(&dev, fmt)?;
        let capture_format = capture_format_for(negotiated.fourcc)?;

        tracing::debug!(
            device,
            width = negotiated.width,
            height = negotiated.height,
            fourcc = ?negotiated.fourcc,
            "camera format negotiated"
        );

        // Set frame interval (best-effort — some cameras ignore it)
        if let Ok(mut params) = dev.params() {
            params.interval = v4l::Fraction::new(1, fps);
            let _ = dev.set_params(&params);
        }

        // SAFETY: the stream stores the device handle internally. We keep the
        // original Device allocated until Drop, where the stream is dropped first.
        let dev_static: &'static Device = Box::leak(Box::new(dev));
        let device_ptr = NonNull::from(dev_static);
        let stream = MmapStream::with_buffers(dev_static, Type::VideoCapture, 4)
            .map_err(|e| FaceRsError::Camera(format!("cannot start stream: {e}")))?;

        Ok(V4lCamera {
            device_path: device.to_owned(),
            stream: Some(stream),
            device: device_ptr,
            width: negotiated.width,
            height: negotiated.height,
            format: capture_format,
            fourcc: negotiated.fourcc,
        })
    }

    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn fourcc(&self) -> FourCC {
        self.fourcc
    }

    /// Discard `n` frames to let the sensor adjust exposure/white-balance.
    pub fn warmup(&mut self, n: u32) {
        for _ in 0..n {
            if let Some(stream) = self.stream.as_mut() {
                let _ = stream.next();
            }
        }
    }

    pub fn capture_frame(&mut self) -> Result<Frame> {
        let (buf, _meta) = self
            .stream
            .as_mut()
            .ok_or_else(|| FaceRsError::Camera("camera stream is closed".to_string()))?
            .next()
            .map_err(|e| FaceRsError::Camera(format!("capture failed: {e}")))?;

        let (rgb, width, height) = match self.format {
            CaptureFormat::Yuyv => (
                yuyv_to_rgb(buf, self.width, self.height)?,
                self.width,
                self.height,
            ),
            CaptureFormat::Mjpeg => mjpeg_to_rgb(buf)?,
            CaptureFormat::Grey => (
                grey_to_rgb(buf, self.width, self.height)?,
                self.width,
                self.height,
            ),
        };

        Ok(Frame {
            data: rgb,
            width,
            height,
        })
    }
}

impl Drop for V4lCamera {
    fn drop(&mut self) {
        let _ = self.stream.take();
        // SAFETY: device was created by Box::leak in open(), and stream has been
        // dropped before reclaiming it.
        unsafe {
            drop(Box::from_raw(self.device.as_ptr()));
        }
    }
}

// ── Format conversions ────────────────────────────────────────────────────────

fn negotiate_format(dev: &Device, mut fmt: v4l::Format) -> Result<v4l::Format> {
    let preferred = [
        FourCC::new(b"MJPG"),
        FourCC::new(b"YUYV"),
        FourCC::new(b"GREY"),
        FourCC::new(b"Y800"),
    ];

    for fourcc in preferred {
        fmt.fourcc = fourcc;
        if let Ok(negotiated) = dev.set_format(&fmt) {
            if capture_format_for(negotiated.fourcc).is_ok() {
                return Ok(negotiated);
            }
        }
    }

    let current = dev
        .format()
        .map_err(|e| FaceRsError::Camera(format!("cannot query negotiated format: {e}")))?;
    capture_format_for(current.fourcc)?;
    Ok(current)
}

fn capture_format_for(fourcc: FourCC) -> Result<CaptureFormat> {
    if fourcc == FourCC::new(b"MJPG") {
        Ok(CaptureFormat::Mjpeg)
    } else if fourcc == FourCC::new(b"YUYV") {
        Ok(CaptureFormat::Yuyv)
    } else if fourcc == FourCC::new(b"GREY") || fourcc == FourCC::new(b"Y800") {
        Ok(CaptureFormat::Grey)
    } else {
        Err(FaceRsError::Camera(format!(
            "unsupported pixel format: {fourcc:?}; supported formats: MJPG, YUYV, GREY, Y800"
        )))
    }
}

fn yuyv_to_rgb(buf: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let expected = (width * height * 2) as usize;
    if buf.len() < expected {
        return Err(FaceRsError::Camera(format!(
            "YUYV buffer too small: {} < {expected}",
            buf.len()
        )));
    }

    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for chunk in buf[..expected].chunks_exact(4) {
        let y0 = chunk[0] as f32;
        let u = chunk[1] as f32 - 128.0;
        let y1 = chunk[2] as f32;
        let v = chunk[3] as f32 - 128.0;
        rgb.extend_from_slice(&yuv_to_rgb_pixel(y0, u, v));
        rgb.extend_from_slice(&yuv_to_rgb_pixel(y1, u, v));
    }
    Ok(rgb)
}

fn grey_to_rgb(buf: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let expected = (width * height) as usize;
    if buf.len() < expected {
        return Err(FaceRsError::Camera(format!(
            "GREY buffer too small: {} < {expected}",
            buf.len()
        )));
    }

    let mut rgb = Vec::with_capacity(expected * 3);
    for &y in &buf[..expected] {
        rgb.extend_from_slice(&[y, y, y]);
    }
    Ok(rgb)
}

#[inline(always)]
fn yuv_to_rgb_pixel(y: f32, u: f32, v: f32) -> [u8; 3] {
    let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
    let g = (y - 0.344_136 * u - 0.714_136 * v).clamp(0.0, 255.0) as u8;
    let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
    [r, g, b]
}

fn mjpeg_to_rgb(buf: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(buf, image::ImageFormat::Jpeg)
        .map_err(|e| FaceRsError::Camera(format!("MJPEG decode failed: {e}")))?;
    let rgb = img.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();
    Ok((rgb.into_raw(), width, height))
}

// ── Stub (tests / CI without a camera) ───────────────────────────────────────

pub struct StubCamera {
    device: String,
    width: u32,
    height: u32,
}

impl StubCamera {
    pub fn open(device: &str, width: u32, height: u32, _fps: u32) -> Result<Self> {
        Ok(StubCamera {
            device: device.to_owned(),
            width,
            height,
        })
    }

    pub fn device_path(&self) -> &str {
        &self.device
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn warmup(&mut self, _n: u32) {}

    pub fn capture_frame(&mut self) -> Result<Frame> {
        Err(FaceRsError::Camera(
            "stub camera: no hardware available".to_string(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_2x1_roundtrip() {
        // A 2×1 YUYV frame: Y0=128 U=128 Y1=128 V=128 → near-grey
        let buf = vec![128u8, 128, 128, 128];
        let rgb = yuyv_to_rgb(&buf, 2, 1).unwrap();
        assert_eq!(rgb.len(), 6);
        // Both pixels should be roughly grey (within 4 of centre)
        for &b in &rgb {
            assert!((b as i16 - 128).abs() < 10, "byte {b} not near grey");
        }
    }

    #[test]
    fn grey_2x1_to_rgb() {
        let rgb = grey_to_rgb(&[10, 240], 2, 1).unwrap();
        assert_eq!(rgb, vec![10, 10, 10, 240, 240, 240]);
    }

    #[test]
    fn stub_open_ok() {
        let mut cam = StubCamera::open("/dev/video0", 640, 360, 30).unwrap();
        assert!(cam.capture_frame().is_err());
    }
}
