use std::path::Path;
use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::detection::ScrfdDetector;
use facegate_core::embedding::ArcFaceEmbedder;

pub fn run(config: &Config) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(config, None, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn run_streaming(
    config: &Config,
    _username: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    out!("Facegate doctor\n");

    let mut all_ok = true;
    all_ok &= chk(
        tx,
        "config file exists",
        Path::new("/etc/facegate/config.toml").exists(),
        None,
    );
    all_ok &= chk(
        tx,
        "PAM module installed",
        Path::new("/usr/lib/security/pam_facegate.so").exists(),
        Some("run: sudo bash install-dev.sh"),
    );
    all_ok &= chk(
        tx,
        "storage base dir exists",
        config.storage.base_dir.exists(),
        Some("run: sudo mkdir -p /var/lib/facegate/users"),
    );
    all_ok &= chk(
        tx,
        "storage permissions are safe",
        safe_dir_permissions(&config.storage.base_dir),
        Some("run: sudo chmod 755 /var/lib/facegate /var/lib/facegate/users"),
    );
    all_ok &= chk(
        tx,
        &format!("camera device  ({})", config.camera.device),
        Path::new(&config.camera.device).exists(),
        Some("check v4l2-ctl --list-devices, then update config"),
    );
    all_ok &= chk(
        tx,
        &format!("detector model  ({})", config.models.detector.display()),
        config.models.detector.exists(),
        Some("run: sudo bash install-dev.sh"),
    );
    all_ok &= chk(
        tx,
        &format!("embedder model  ({})", config.models.embedder.display()),
        config.models.embedder.exists(),
        Some("run: sudo bash install-dev.sh"),
    );

    let ort_ok = [
        "/usr/lib/libonnxruntime.so",
        "/usr/lib/libonnxruntime.so.1",
        "/usr/local/lib/libonnxruntime.so",
        "/usr/local/lib/libonnxruntime.so.1",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    all_ok &= chk(
        tx,
        "ONNX Runtime library",
        ort_ok,
        Some("sudo pacman -S onnxruntime"),
    );

    if config.models.detector.exists() {
        all_ok &= chk(
            tx,
            "detector model loads",
            ScrfdDetector::load(&config.models.detector).is_ok(),
            Some("check ONNX Runtime version and detector model file"),
        );
    }
    if config.models.embedder.exists() {
        all_ok &= chk(
            tx,
            "embedder model loads",
            ArcFaceEmbedder::load(&config.models.embedder).is_ok(),
            Some("check ONNX Runtime version and embedder model file"),
        );
    }

    // Check that the watch service unit is installed.
    let service_installed = Path::new("/usr/lib/systemd/user/facegate-watch.service").exists()
        || Path::new("/etc/systemd/user/facegate-watch.service").exists();
    all_ok &= chk(
        tx,
        "facegate-watch.service installed",
        service_installed,
        Some("run: sudo bash install-dev.sh  (or reinstall the package)"),
    );

    out!("");
    if all_ok {
        out!("All checks passed.");
    } else {
        out!("Some checks failed — see hints above.");
    }
    Ok(())
}

fn safe_dir_permissions(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    meta.permissions().mode() & 0o022 == 0
}

fn chk(tx: &Sender<String>, label: &str, ok: bool, hint: Option<&str>) -> bool {
    let mark = if ok { "✓" } else { "✗" };
    let _ = tx.send(format!("  [{mark}] {label}"));
    if !ok {
        if let Some(h) = hint {
            let _ = tx.send(format!("        → {h}"));
        }
    }
    ok
}
