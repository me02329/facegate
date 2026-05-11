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
        safe_template_storage(&config.storage.base_dir),
        Some("run: sudo chown -R facegate:facegate /var/lib/facegate/users && sudo chmod 700 /var/lib/facegate/users"),
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

fn safe_template_storage(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Ok(facegate_uid) = uid_for_user("facegate") else {
        return false;
    };
    let Some(facegate_uid) = facegate_uid else {
        return false;
    };
    let Ok(facegate_gid) = gid_for_group("facegate") else {
        return false;
    };
    let Some(facegate_gid) = facegate_gid else {
        return false;
    };

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    if meta.uid() != facegate_uid || meta.gid() != facegate_gid {
        return false;
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return false;
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if meta.file_type().is_symlink() || meta.uid() != facegate_uid || meta.gid() != facegate_gid
        {
            return false;
        }
        let mode = meta.permissions().mode() & 0o777;
        if meta.file_type().is_dir() {
            if mode & 0o077 != 0 {
                return false;
            }
            let embeddings = entry.path().join("embeddings.json");
            if embeddings.exists() {
                let Ok(file_meta) = std::fs::symlink_metadata(&embeddings) else {
                    return false;
                };
                if !file_meta.file_type().is_file()
                    || file_meta.file_type().is_symlink()
                    || file_meta.uid() != facegate_uid
                    || file_meta.gid() != facegate_gid
                    || file_meta.permissions().mode() & 0o077 != 0
                {
                    return false;
                }
            }
        } else if meta.file_type().is_file() {
            if mode & 0o077 != 0 {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

fn uid_for_user(username: &str) -> anyhow::Result<Option<u32>> {
    let c_name = std::ffi::CString::new(username)?;
    let mut buf = vec![0i8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc).into());
    }
    Ok((!result.is_null()).then_some(pwd.pw_uid))
}

fn gid_for_group(group: &str) -> anyhow::Result<Option<u32>> {
    let c_name = std::ffi::CString::new(group)?;
    let mut buf = vec![0i8; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    let rc = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc).into());
    }
    Ok((!result.is_null()).then_some(grp.gr_gid))
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
