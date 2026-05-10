use std::path::Path;
use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::detection::ScrfdDetector;
use facegate_core::embedding::ArcFaceEmbedder;
use v4l::video::Capture;
use v4l::Device;

#[derive(Debug, Clone, Copy)]
enum CameraKind {
    Ir,
    Rgb,
    Unknown,
}

fn camera_kind(device: &str) -> CameraKind {
    let Ok(dev) = Device::with_path(device) else {
        return CameraKind::Unknown;
    };
    let Ok(formats) = dev.enum_formats() else {
        return CameraKind::Unknown;
    };
    let mut has_ir = false;
    let mut has_rgb = false;
    for f in &formats {
        match f.fourcc.to_string().as_str() {
            "GREY" | "Y8  " | "Y800" => has_ir = true,
            "YUYV" | "MJPG" => has_rgb = true,
            _ => {}
        }
    }
    if has_ir {
        CameraKind::Ir
    } else if has_rgb {
        CameraKind::Rgb
    } else {
        CameraKind::Unknown
    }
}

#[derive(Debug, Clone, Copy)]
enum Distro {
    Arch,
    Debian,
    Fedora,
    OpenSuse,
    Other,
}

fn detect_distro() -> Distro {
    let Ok(content) = std::fs::read_to_string("/etc/os-release") else {
        return Distro::Other;
    };
    let id_like = content
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k, v.trim_matches('"').to_ascii_lowercase()))
        .collect::<Vec<_>>();
    let get = |key: &str| {
        id_like
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    };
    let id = get("ID").unwrap_or("");
    let id_like = get("ID_LIKE").unwrap_or("");
    let combined = format!("{id} {id_like}");
    if combined.contains("arch") {
        Distro::Arch
    } else if combined.contains("debian") || combined.contains("ubuntu") {
        Distro::Debian
    } else if combined.contains("fedora")
        || combined.contains("rhel")
        || combined.contains("centos")
    {
        Distro::Fedora
    } else if combined.contains("suse") || combined.contains("opensuse") {
        Distro::OpenSuse
    } else {
        Distro::Other
    }
}

fn ort_install_hint(distro: Distro) -> &'static str {
    match distro {
        Distro::Arch => "sudo pacman -S onnxruntime",
        Distro::Debian => "sudo apt install libonnxruntime-dev (or run the postinstall download)",
        Distro::Fedora => "sudo dnf install onnxruntime (or run the postinstall download)",
        Distro::OpenSuse => "sudo zypper install onnxruntime",
        Distro::Other => {
            "install onnxruntime via your package manager, or rerun the package postinstall"
        }
    }
}

fn reinstall_hint(distro: Distro) -> &'static str {
    match distro {
        Distro::Arch => "reinstall the package: sudo pacman -U facegate-*.pkg.tar.zst",
        Distro::Debian => "reinstall the package: sudo apt install ./facegate_*.deb",
        Distro::Fedora => "reinstall the package: sudo dnf install ./facegate-*.rpm",
        Distro::OpenSuse => "reinstall the package: sudo zypper install ./facegate-*.rpm",
        Distro::Other => "reinstall the package, or run install-dev.sh from the source tree",
    }
}

pub fn run(config: &Config) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = config.clone();
    let handle = std::thread::spawn(move || run_streaming(&config, None, &tx));

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
    _username: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    out!("Facegate doctor\n");

    let distro = detect_distro();
    let mut all_ok = true;
    all_ok &= chk(
        tx,
        "config file exists",
        Path::new("/etc/facegate/config.toml").exists(),
        None,
    );
    let pam_paths = [
        "/usr/lib/security/pam_facegate.so",
        "/usr/lib/x86_64-linux-gnu/security/pam_facegate.so",
        "/usr/lib64/security/pam_facegate.so",
        "/lib/security/pam_facegate.so",
    ];
    let pam_found = pam_paths.iter().any(|p| Path::new(p).exists());
    all_ok &= chk(
        tx,
        "PAM module installed",
        pam_found,
        Some(reinstall_hint(distro)),
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
        Some("run: facegate cameras  (no root needed) to find the right device"),
    );

    // Help users on dual-camera laptops realise they picked the RGB webcam
    // when the IR sensor would be more secure. We don't fail doctor for it —
    // it's only a recommendation.
    if Path::new(&config.camera.device).exists() {
        match camera_kind(&config.camera.device) {
            CameraKind::Ir => {
                let _ = tx.send("        → IR camera detected (recommended)".to_owned());
            }
            CameraKind::Rgb => {
                let _ = tx.send(
                    "        → RGB webcam — works, but IR is more robust against \
                     photo spoofing. Run: facegate cameras"
                        .to_owned(),
                );
            }
            CameraKind::Unknown => {}
        }
    }
    all_ok &= chk(
        tx,
        &format!("detector model  ({})", config.models.detector.display()),
        config.models.detector.exists(),
        Some(reinstall_hint(distro)),
    );
    all_ok &= chk(
        tx,
        &format!("embedder model  ({})", config.models.embedder.display()),
        config.models.embedder.exists(),
        Some(reinstall_hint(distro)),
    );

    let ort_ok = [
        "/usr/lib/libonnxruntime.so",
        "/usr/lib/libonnxruntime.so.1",
        "/usr/lib/x86_64-linux-gnu/libonnxruntime.so",
        "/usr/lib/x86_64-linux-gnu/libonnxruntime.so.1",
        "/usr/lib64/libonnxruntime.so",
        "/usr/lib64/libonnxruntime.so.1",
        "/usr/local/lib/libonnxruntime.so",
        "/usr/local/lib/libonnxruntime.so.1",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    all_ok &= chk(
        tx,
        "ONNX Runtime library",
        ort_ok,
        Some(ort_install_hint(distro)),
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
        Some(reinstall_hint(distro)),
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
