use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use facegate_core::camera::{Frame, V4lCamera};
use facegate_core::config::{CameraIrConfig, Config};
use facegate_core::detection::{Detection, ScrfdDetector};
use facegate_core::error::FaceRsError;

use crate::commands::broker;
use crate::commands::services;

const LANDMARKS_PER_FACE: usize = 5;
const HOMOGRAPHY_PARAMS: usize = 8;

#[derive(Debug, Clone)]
pub struct CameraCalibration {
    pub homography: [f32; 9],
    pub rms_error_px: f32,
    pub max_error_px: f32,
    pub pairs: usize,
}

pub fn run(
    mut config: Config,
    config_path: PathBuf,
    rgb_device: Option<&str>,
    ir_device: Option<&str>,
    samples: u32,
    write: bool,
    enable: bool,
) -> anyhow::Result<()> {
    if samples == 0 {
        bail!("--samples must be greater than zero");
    }

    let rgb_device = rgb_device.unwrap_or(&config.camera.device).to_owned();
    let ir_device = ir_device
        .or_else(|| config.camera.ir.as_ref().map(|ir| ir.device.as_str()))
        .ok_or_else(|| {
            anyhow::anyhow!("IR device missing; pass --ir-device or set [camera.ir].device")
        })?
        .to_owned();

    if rgb_device == ir_device {
        bail!("RGB and IR devices must be different");
    }

    println!("Facegate RGB+IR camera calibration");
    println!("RGB device : {rgb_device}");
    println!("IR device  : {ir_device}");
    println!("Samples    : {samples}");
    println!();
    println!("Keep your face visible in both streams. Each accepted capture must contain exactly one face in RGB and IR.");
    println!();

    // Open the RGB camera with the top-level settings, and the IR camera with
    // whatever overrides the operator has already set (or IR-friendly
    // defaults). This matches the auth/watch paths so calibration runs against
    // the same resolutions the broker will see at runtime.
    let mut rgb = broker::open_rgb_camera(&config).context("cannot open RGB camera")?;
    let mut ir = open_ir_for_calibration(&config, &ir_device).context("cannot open IR camera")?;
    let mut detector = ScrfdDetector::load(&config.models.detector).with_context(|| {
        format!(
            "cannot load detector at {}",
            config.models.detector.display()
        )
    })?;
    let ir_min_face_size = config
        .camera
        .ir
        .as_ref()
        .map(|ir| ir.effective_min_face_size(config.recognition.min_face_size))
        .unwrap_or(config.recognition.min_face_size);

    let mut correspondences = Vec::with_capacity(samples as usize * LANDMARKS_PER_FACE);
    // Cap on *consecutive* failed captures rather than total attempts: a user
    // who already collected a few good pairs shouldn't have to start over
    // because of a bad streak. Reset on every accepted pair.
    const MAX_CONSECUTIVE_FAILURES: u32 = 12;
    let mut consecutive_failures: u32 = 0;

    while correspondences.len() < samples as usize * LANDMARKS_PER_FACE {
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            bail!(
                "{} consecutive captures failed without a usable RGB+IR pair (got {}/{}). \
                 Try better lighting, sit ~50 cm from the camera with your face centred, \
                 or lower `recognition.min_face_size` temporarily.",
                MAX_CONSECUTIVE_FAILURES,
                correspondences.len() / LANDMARKS_PER_FACE,
                samples
            );
        }

        wait_for_enter(
            correspondences.len() / LANDMARKS_PER_FACE + 1,
            samples as usize,
        )?;
        let (rgb_result, ir_result) = broker::capture_rgb_ir_pair(&mut rgb, &mut ir);
        let rgb_frame = rgb_result.context("RGB capture failed")?;
        let ir_frame = ir_result.context("IR capture failed")?;

        let rgb_detection =
            match one_face(&mut detector, &rgb_frame, config.recognition.min_face_size) {
                Ok(detection) => detection,
                Err(e) => {
                    consecutive_failures += 1;
                    println!(
                        "  skipped: RGB {e} ({}/{} before bail)",
                        consecutive_failures, MAX_CONSECUTIVE_FAILURES
                    );
                    continue;
                }
            };
        let ir_detection = match one_face(&mut detector, &ir_frame, ir_min_face_size) {
            Ok(detection) => detection,
            Err(e) => {
                consecutive_failures += 1;
                println!(
                    "  skipped: IR {e} ({}/{} before bail)",
                    consecutive_failures, MAX_CONSECUTIVE_FAILURES
                );
                continue;
            }
        };

        consecutive_failures = 0;
        for i in 0..LANDMARKS_PER_FACE {
            correspondences.push(Correspondence {
                ir: ir_detection.landmarks.points[i],
                rgb: rgb_detection.landmarks.points[i],
            });
        }
        println!(
            "  accepted pair {}/{}",
            correspondences.len() / LANDMARKS_PER_FACE,
            samples
        );
    }

    let calibration = calibrate_homography(&correspondences)?;
    println!();
    println!("Calibration result");
    println!("  landmark pairs : {}", calibration.pairs);
    println!("  RMS error      : {:.2}px", calibration.rms_error_px);
    println!("  Max error      : {:.2}px", calibration.max_error_px);
    println!(
        "  homography     : {}",
        format_homography(&calibration.homography)
    );

    if write {
        println!();
        if ask_yes_no(
            &format!(
                "Write RGB+IR calibration to {}{}?",
                config_path.display(),
                if enable {
                    " and enable cross-check"
                } else {
                    ""
                }
            ),
            false,
        )? {
            backup_config(&config_path)?;
            config.camera.device = rgb_device;
            // Preserve any IR-specific overrides the operator already set;
            // only fill in the device path if the section is missing.
            match config.camera.ir.as_mut() {
                Some(ir) => ir.device = ir_device,
                None => {
                    config.camera.ir = Some(CameraIrConfig {
                        device: ir_device,
                        width: None,
                        height: None,
                        fps: None,
                        timeout_ms: None,
                        warmup_frames: None,
                        min_face_size: None,
                    });
                }
            }
            config.camera.cross_check.homography = calibration.homography;
            if enable {
                config.camera.cross_check.enabled = true;
            }
            write_config(&config_path, &config)?;
            println!("Config updated.");
            services::print_refresh_summary(&services::refresh_after_config_change());
        } else {
            println!("Config unchanged.");
        }
    } else {
        println!();
        println!("Config unchanged. Re-run with --write to save the homography.");
    }

    Ok(())
}

fn open_ir_for_calibration(
    config: &Config,
    device_override: &str,
) -> std::result::Result<V4lCamera, FaceRsError> {
    // Calibration may run with --ir-device pointing at a device that is not
    // yet listed in [camera.ir]; honour the override but use the IR-specific
    // settings (warmup, timeout, resolution) from the config when present.
    let (width, height, fps, timeout_ms, warmup_frames) =
        if let Some(ir) = config.camera.ir.as_ref() {
            (
                ir.effective_width(config.camera.width),
                ir.effective_height(config.camera.height),
                ir.effective_fps(config.camera.fps),
                // Calibration is interactive; allow a generous floor so a slow
                // IR sensor doesn't kill the session before the user presses
                // Enter again.
                ir.effective_timeout_ms(config.camera.timeout_ms)
                    .max(15_000),
                ir.effective_warmup_frames(config.camera.warmup_frames),
            )
        } else {
            (
                config.camera.width,
                config.camera.height,
                config.camera.fps,
                config.camera.timeout_ms.max(15_000),
                config.camera.warmup_frames.max(10),
            )
        };
    let mut camera = V4lCamera::open(device_override, width, height, fps, timeout_ms)?;
    camera.warmup(warmup_frames);
    Ok(camera)
}

fn one_face(
    detector: &mut ScrfdDetector,
    frame: &Frame,
    min_size: u32,
) -> anyhow::Result<Detection> {
    let detections = detector.detect(frame, min_size)?;
    match detections.len() {
        1 => Ok(detections.into_iter().next().expect("len checked")),
        0 => bail!("detected no face"),
        n => bail!("detected {n} faces"),
    }
}

#[derive(Debug, Clone, Copy)]
struct Correspondence {
    ir: (f32, f32),
    rgb: (f32, f32),
}

fn calibrate_homography(points: &[Correspondence]) -> anyhow::Result<CameraCalibration> {
    if points.len() < 4 {
        bail!("at least four point correspondences are required");
    }

    let mut normal = [[0.0f64; HOMOGRAPHY_PARAMS]; HOMOGRAPHY_PARAMS];
    let mut rhs = [0.0f64; HOMOGRAPHY_PARAMS];

    for point in points {
        let x = f64::from(point.ir.0);
        let y = f64::from(point.ir.1);
        let u = f64::from(point.rgb.0);
        let v = f64::from(point.rgb.1);
        accumulate_row(
            &mut normal,
            &mut rhs,
            [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y],
            u,
        );
        accumulate_row(
            &mut normal,
            &mut rhs,
            [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y],
            v,
        );
    }

    let solution =
        solve_8x8(normal, rhs).context("cannot solve homography; captures may be degenerate")?;
    let homography = [
        solution[0] as f32,
        solution[1] as f32,
        solution[2] as f32,
        solution[3] as f32,
        solution[4] as f32,
        solution[5] as f32,
        solution[6] as f32,
        solution[7] as f32,
        1.0,
    ];

    let (rms_error_px, max_error_px) = reprojection_error(points, &homography)?;
    Ok(CameraCalibration {
        homography,
        rms_error_px,
        max_error_px,
        pairs: points.len(),
    })
}

fn accumulate_row(
    normal: &mut [[f64; HOMOGRAPHY_PARAMS]; HOMOGRAPHY_PARAMS],
    rhs: &mut [f64; HOMOGRAPHY_PARAMS],
    row: [f64; HOMOGRAPHY_PARAMS],
    target: f64,
) {
    for r in 0..HOMOGRAPHY_PARAMS {
        rhs[r] += row[r] * target;
        for c in 0..HOMOGRAPHY_PARAMS {
            normal[r][c] += row[r] * row[c];
        }
    }
}

fn solve_8x8(
    mut matrix: [[f64; HOMOGRAPHY_PARAMS]; HOMOGRAPHY_PARAMS],
    mut rhs: [f64; HOMOGRAPHY_PARAMS],
) -> Option<[f64; HOMOGRAPHY_PARAMS]> {
    for pivot in 0..HOMOGRAPHY_PARAMS {
        let mut best = pivot;
        for row in (pivot + 1)..HOMOGRAPHY_PARAMS {
            if matrix[row][pivot].abs() > matrix[best][pivot].abs() {
                best = row;
            }
        }
        if matrix[best][pivot].abs() < 1e-9 {
            return None;
        }
        if best != pivot {
            matrix.swap(best, pivot);
            rhs.swap(best, pivot);
        }

        let scale = matrix[pivot][pivot];
        for value in matrix[pivot].iter_mut().skip(pivot) {
            *value /= scale;
        }
        rhs[pivot] /= scale;
        let pivot_row = matrix[pivot];

        for row in 0..HOMOGRAPHY_PARAMS {
            if row == pivot {
                continue;
            }
            let factor = matrix[row][pivot];
            for (col, value) in matrix[row].iter_mut().enumerate().skip(pivot) {
                *value -= factor * pivot_row[col];
            }
            rhs[row] -= factor * rhs[pivot];
        }
    }
    Some(rhs)
}

fn reprojection_error(
    points: &[Correspondence],
    homography: &[f32; 9],
) -> anyhow::Result<(f32, f32)> {
    let mut squared_sum = 0.0f32;
    let mut max_error = 0.0f32;
    for point in points {
        let mapped = apply_homography(point.ir, homography)
            .ok_or_else(|| anyhow::anyhow!("homography maps a point to infinity"))?;
        let dx = mapped.0 - point.rgb.0;
        let dy = mapped.1 - point.rgb.1;
        let error = (dx * dx + dy * dy).sqrt();
        squared_sum += error * error;
        max_error = max_error.max(error);
    }
    Ok(((squared_sum / points.len() as f32).sqrt(), max_error))
}

fn apply_homography(point: (f32, f32), h: &[f32; 9]) -> Option<(f32, f32)> {
    let (x, y) = point;
    let denom = h[6] * x + h[7] * y + h[8];
    if !denom.is_finite() || denom.abs() < f32::EPSILON {
        return None;
    }
    let mapped_x = (h[0] * x + h[1] * y + h[2]) / denom;
    let mapped_y = (h[3] * x + h[4] * y + h[5]) / denom;
    (mapped_x.is_finite() && mapped_y.is_finite()).then_some((mapped_x, mapped_y))
}

fn format_homography(h: &[f32; 9]) -> String {
    format!(
        "[{:.6}, {:.6}, {:.6}, {:.6}, {:.6}, {:.6}, {:.8}, {:.8}, {:.6}]",
        h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]
    )
}

fn wait_for_enter(index: usize, total: usize) -> anyhow::Result<()> {
    print!("Press Enter to capture RGB+IR pair {index}/{total}...");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(())
}

fn ask_yes_no(prompt: &str, default_yes: bool) -> anyhow::Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {suffix} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    match trimmed.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => bail!("expected yes or no"),
    }
}

fn backup_config(config_path: &Path) -> anyhow::Result<()> {
    if !config_path.exists() {
        return Ok(());
    }
    let backup = config_path.with_extension(format!(
        "{}.bak",
        config_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("toml")
    ));
    std::fs::copy(config_path, &backup).with_context(|| {
        format!(
            "cannot back up {} to {}",
            config_path.display(),
            backup.display()
        )
    })?;
    println!("Backed up config to {}", backup.display());
    Ok(())
}

fn write_config(config_path: &Path, config: &Config) -> anyhow::Result<()> {
    let parent = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", config_path.display()))?;
    std::fs::create_dir_all(parent)?;
    let toml = toml::to_string_pretty(config)?;
    std::fs::write(config_path, toml)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_solves_translation() {
        let points = square_points(12.0, -7.0);
        let calibration = calibrate_homography(&points).unwrap();
        assert!((calibration.homography[0] - 1.0).abs() < 0.0001);
        assert!(calibration.homography[1].abs() < 0.0001);
        assert!((calibration.homography[2] - 12.0).abs() < 0.0001);
        assert!(calibration.homography[3].abs() < 0.0001);
        assert!((calibration.homography[4] - 1.0).abs() < 0.0001);
        assert!((calibration.homography[5] + 7.0).abs() < 0.0001);
        assert!(calibration.rms_error_px < 0.001);
    }

    #[test]
    fn homography_solves_scale_and_translation() {
        let mut points = Vec::new();
        for (x, y) in [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 80.0),
            (0.0, 80.0),
            (50.0, 40.0),
        ] {
            points.push(Correspondence {
                ir: (x, y),
                rgb: (x * 1.2 + 4.0, y * 0.8 - 3.0),
            });
        }
        let calibration = calibrate_homography(&points).unwrap();
        assert!((calibration.homography[0] - 1.2).abs() < 0.0001);
        assert!((calibration.homography[2] - 4.0).abs() < 0.0001);
        assert!((calibration.homography[4] - 0.8).abs() < 0.0001);
        assert!((calibration.homography[5] + 3.0).abs() < 0.0001);
        assert!(calibration.max_error_px < 0.001);
    }

    #[test]
    fn homography_requires_enough_points() {
        assert!(calibrate_homography(&[]).is_err());
    }

    fn square_points(dx: f32, dy: f32) -> Vec<Correspondence> {
        [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
            .into_iter()
            .map(|ir| Correspondence {
                ir,
                rgb: (ir.0 + dx, ir.1 + dy),
            })
            .collect()
    }
}
