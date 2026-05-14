//! Enumerate `/dev/video*` devices and print their key properties so the
//! user can pick the right one — especially the IR / depth camera that
//! Windows-Hello-style hardware exposes alongside the regular RGB webcam.

use std::path::Path;
use std::sync::mpsc::Sender;

use v4l::video::Capture;
use v4l::{Device, FourCC};

pub fn run() -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || run_streaming(&tx));

    for line in rx {
        println!("{line}");
    }

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("thread panicked"))??;
    Ok(())
}

pub fn run_streaming(tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir("/dev")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("video") && n[5..].chars().all(|c| c.is_ascii_digit())
            })
        })
        .collect();
    paths.sort();

    if paths.is_empty() {
        out!("No /dev/video* devices found.");
        out!("");
        out!("If you're inside a container, mount /dev/video* into it; otherwise");
        out!("check that the uvcvideo / IR-camera kernel module is loaded.");
        return Ok(());
    }

    out!("Detected video devices:");
    out!("");

    let mut recommended = RecommendedCameras::default();

    for path in &paths {
        describe_device(tx, path, &mut recommended);
    }

    out!("");
    out!("Legend:  RGB = colour stream (YUYV / MJPG)  — primary camera for face matching");
    out!("         IR  = grayscale stream (Y8 / GREY) — optional, used as a liveness");
    out!("                                              signal in RGB+IR cross-check");
    out!("");

    match (&recommended.rgb, &recommended.ir) {
        (Some(rgb), Some(ir)) => {
            out!("Recommended:");
            out!("  [camera].device      = \"{rgb}\"   (RGB — primary)");
            out!("  [camera.ir].device   = \"{ir}\"   (IR — cross-check)");
            out!("");
            out!("Quickest path:");
            out!("  sudo facegate setup            # picks both + offers calibration");
            out!("  sudo facegate calibrate-cameras --write --enable");
        }
        (Some(rgb), None) => {
            out!("Recommended:");
            out!("  [camera].device = \"{rgb}\"   (RGB — primary)");
            out!("");
            out!("No IR sensor detected, so RGB+IR cross-check is unavailable on");
            out!("this hardware. Set the device with:");
            out!("  sudo facegate configure        # edit [camera].device");
        }
        (None, Some(ir)) => {
            out!("Only an IR sensor was detected ({ir}); facegate needs an RGB");
            out!("camera as the primary device. Plug in a webcam and re-run, or");
            out!("set [camera].device manually to whichever node speaks YUYV/MJPG.");
        }
        (None, None) => {
            out!("No device exposes a capture format we can use.");
            out!("Run `v4l2-ctl --list-devices` to inspect them manually.");
        }
    }

    Ok(())
}

#[derive(Default)]
struct RecommendedCameras {
    rgb: Option<String>,
    ir: Option<String>,
}

fn describe_device(tx: &Sender<String>, path: &Path, recommended: &mut RecommendedCameras) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    let path_str = path.display().to_string();

    let dev = match Device::with_path(path) {
        Ok(d) => d,
        Err(e) => {
            out!("  {path_str}");
            out!("    ✗ cannot open: {e}");
            return;
        }
    };

    let caps = dev.query_caps().ok();
    let card = caps
        .as_ref()
        .map(|c| c.card.as_str())
        .unwrap_or("(unknown)");
    let driver = caps
        .as_ref()
        .map(|c| c.driver.as_str())
        .unwrap_or("(unknown)");

    let formats = dev.enum_formats().unwrap_or_default();
    let fourccs: Vec<String> = formats.iter().map(|f| f.fourcc.to_string()).collect();
    let is_ir = formats.iter().any(|f| {
        let s = f.fourcc.to_string();
        matches!(s.as_str(), "GREY" | "Y8  " | "Y800")
    });
    let is_rgb = formats
        .iter()
        .any(|f| matches!(f.fourcc.to_string().as_str(), "YUYV" | "MJPG"));

    let kind_tag = match (is_ir, is_rgb) {
        (true, _) => "IR",
        (false, true) => "RGB",
        (false, false) => "?",
    };

    out!("  [{kind_tag}] {path_str}  —  {card} ({driver})");
    if formats.is_empty() {
        out!("        no capture formats reported (probably a metadata node)");
        return;
    }
    out!("        formats: {}", fourccs.join(", "));

    // Record the first plain-RGB and the first IR-only node we find. Dual-mode
    // sensors that report both YUYV/MJPG and GREY are treated as RGB primaries
    // (the colour stream is the useful one); pure IR nodes go to camera.ir.
    if is_ir && !is_rgb && recommended.ir.is_none() {
        recommended.ir = Some(path_str.clone());
    } else if is_rgb && recommended.rgb.is_none() {
        recommended.rgb = Some(path_str.clone());
    }

    // Probe a default capture format to detect non-capture nodes (some
    // /dev/videoN are M2M / metadata only). FourCC detection above already
    // filters, but a one-line warning for "open OK, no Capture" helps.
    if !is_capture_capable(&dev) {
        out!("        note: device is not a video-capture node");
    }
}

fn is_capture_capable(dev: &Device) -> bool {
    // Try the canonical capture format query — if it errors, the device
    // doesn't speak V4L2_BUF_TYPE_VIDEO_CAPTURE.
    Capture::format(dev).is_ok()
        || dev.enum_formats().map(|fs| !fs.is_empty()).unwrap_or(false)
            && dev
                .enum_formats()
                .ok()
                .and_then(|fs| fs.first().map(|f| f.fourcc))
                .is_some_and(|cc| cc != FourCC::new(b"\0\0\0\0"))
}
