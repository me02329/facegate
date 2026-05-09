use std::path::Path;
use std::sync::mpsc::Sender;

use facegate_core::config::Config;

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
        "/usr/local/lib/libonnxruntime.so",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    all_ok &= chk(
        tx,
        "ONNX Runtime library",
        ort_ok,
        Some("sudo pacman -S onnxruntime"),
    );

    out!("");
    if all_ok {
        out!("All checks passed.");
    } else {
        out!("Some checks failed — see hints above.");
    }
    Ok(())
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
