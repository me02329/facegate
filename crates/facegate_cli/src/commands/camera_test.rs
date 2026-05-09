use std::sync::mpsc::Sender;

use facegate_core::camera::V4lCamera;
use facegate_core::config::Config;
use facegate_core::detection::ScrfdDetector;

pub fn run(config: &Config, device_override: Option<&str>) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = config.clone();
    let device_override = device_override.map(str::to_owned);

    let handle =
        std::thread::spawn(move || run_streaming(&config, device_override.as_deref(), &tx));

    for line in rx {
        println!("{line}");
    }

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("thread panicked"))??;
    Ok(())
}

pub fn run_streaming(
    config: &Config,
    device_override: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let device = device_override.unwrap_or(&config.camera.device);
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    out!("Opening camera: {device}");
    let mut camera = V4lCamera::open(
        device,
        config.camera.width,
        config.camera.height,
        config.camera.fps,
        config.camera.timeout_ms,
    )?;
    out!("  resolution : {}×{}", camera.width(), camera.height());
    out!("  format     : {}", camera.fourcc());

    out!("  Warming up ({} frames)...", config.camera.warmup_frames);
    camera.warmup(config.camera.warmup_frames);

    out!("  Capturing frame...");
    let frame = camera.capture_frame()?;
    out!(
        "  captured   : {}×{} ({} bytes)",
        frame.width,
        frame.height,
        frame.data.len()
    );

    if config.models.detector.exists() {
        out!("  Loading detector...");
        let mut detector = ScrfdDetector::load(&config.models.detector)?;
        out!("  Detector loaded.");
        out!("  Detecting face...");
        let dets = detector.detect(&frame, config.recognition.min_face_size)?;

        if dets.is_empty() {
            out!("  face found : NO");
        } else {
            out!("  face found : YES ({} face(s))", dets.len());
            for (i, d) in dets.iter().enumerate() {
                out!(
                    "    [{i}] conf={:.2}  bbox=[{:.0},{:.0},{:.0},{:.0}]",
                    d.bbox.confidence,
                    d.bbox.x1,
                    d.bbox.y1,
                    d.bbox.x2,
                    d.bbox.y2
                );
            }
        }
    } else {
        out!(
            "  detector model not found at {} — skipping detection",
            config.models.detector.display()
        );
    }
    Ok(())
}
