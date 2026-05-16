use std::path::Path;
use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::detection::ScrfdDetector;
use facegate_core::embedding::ArcFaceEmbedder;
use facegate_core::storage::AuthScope;
use v4l::video::Capture;
use v4l::Device;

use crate::commands::auth::auth_budget;

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
    if config.camera.cross_check.enabled {
        let ir_device: Option<&str> = config.camera.ir.as_ref().map(|ir| ir.device.as_str());
        let ir_ok = ir_device.map(|d| Path::new(d).exists()).unwrap_or(false);
        all_ok &= chk(
            tx,
            &format!("IR camera device ({})", ir_device.unwrap_or("<missing>")),
            ir_ok,
            Some("set [camera.ir].device to the IR / GREY camera, or disable [camera.cross_check]"),
        );
    }

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
    let detector_present = config.models.detector.exists();
    let detector_hint: Option<&str> =
        if !detector_present && legacy_v03_model_path(&config.models.detector) {
            Some(
                "config still points at the v0.3.x detector. Edit [models].detector \
             in /etc/facegate/config.toml to:\n          \
             /usr/share/facegate/models/face_detection_yunet_2023mar.onnx\n        \
             (the v0.4.0 default is YuNet under MIT; run 'sudo facegate doctor' \
             after editing.)",
            )
        } else {
            Some(reinstall_hint(distro))
        };
    all_ok &= chk(
        tx,
        &format!("detector model  ({})", config.models.detector.display()),
        detector_present,
        detector_hint,
    );

    let embedder_present = config.models.embedder.exists();
    let embedder_hint: Option<&str> =
        if !embedder_present && legacy_v03_model_path(&config.models.embedder) {
            Some(
                "config still points at the v0.3.x embedder. Edit [models].embedder \
             in /etc/facegate/config.toml to:\n          \
             /usr/share/facegate/models/glintr100.onnx\n        \
             (the v0.4.0 default is AuraFace under Apache 2.0; all existing \
             templates will need re-enrolment.)",
            )
        } else {
            Some(reinstall_hint(distro))
        };
    all_ok &= chk(
        tx,
        &format!("embedder model  ({})", config.models.embedder.display()),
        embedder_present,
        embedder_hint,
    );

    // Surface the v0.3.x → v0.4.0 template migration. The new embedder
    // (AuraFace / glintr100) produces 512-d vectors in a different latent
    // space than the old InsightFace ArcFace; templates enrolled before
    // the swap will never match no matter how good the face capture is.
    if let Some(stale) = legacy_user_dirs(&config.storage.base_dir, &config.models.embedder) {
        if !stale.is_empty() {
            out!(
                "WARNING: {} user(s) have templates enrolled before the v0.4.0 \
                 embedder swap:",
                stale.len()
            );
            for user in &stale {
                out!("           - {user}");
            }
            out!(
                "         Re-enrol with 'sudo facegate add --user <name>' for each \
                 listed user;\n         the old templates will never match against the \
                 new embedder's embedding space."
            );
            // Not an `all_ok` fail — the broker still loads and the user can
            // still password-fallback. It's a migration nudge, not a hard error.
        }
    }

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

    // Surface the worst-case auth wait so an operator can see at a glance
    // whether the configured policy is going to make face auth feel slow
    // before the password prompt kicks in. The helper bails itself at
    // this budget — the PAM module's safety net is set far above it.
    let sudo_budget = auth_budget(config, &config.recognition.policy_for(AuthScope::Sudo));
    let session_budget = auth_budget(config, &config.recognition.policy_for(AuthScope::Session));
    out!("");
    out!(
        "worst-case auth wait before password fallback: sudo {:.1}s, session {:.1}s",
        sudo_budget.as_secs_f32(),
        session_budget.as_secs_f32(),
    );
    if sudo_budget.as_secs() > 30 {
        out!(
            "        → sudo budget is long; lower [recognition.sudo].max_attempts \
             or [camera].timeout_ms if face auth feels sluggish"
        );
    }

    out!("");
    if all_ok {
        out!("All checks passed.");
    } else {
        out!("Some checks failed — see hints above.");
    }
    Ok(())
}

/// Returns true if `path` ends in one of the model filenames that
/// v0.3.x shipped — used to give a targeted migration hint when the
/// user's config still points at the InsightFace bundle filenames after
/// upgrading to v0.4.0.
fn legacy_v03_model_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    matches!(
        name,
        "scrfd_500m.onnx"
            | "arcface_w600k_r50.onnx"
            | "det_500m.onnx"
            | "w600k_r50.onnx"
            | "det_10g.onnx"
            | "w600k_mbf.onnx"
    )
}

/// Returns the list of usernames whose `embeddings.json` predates the
/// configured embedder model file. The heuristic catches the common
/// v0.3.x → v0.4.0 upgrade pattern: enrol on the old InsightFace
/// embedder, then upgrade and have postinstall drop a newer embedder
/// file into `/usr/share/facegate/models/`. Returns `None` if either
/// directory is unreadable — silence is the right behaviour for a
/// best-effort migration nudge.
fn legacy_user_dirs(base_dir: &Path, embedder_path: &Path) -> Option<Vec<String>> {
    let embedder_mtime = std::fs::metadata(embedder_path).ok()?.modified().ok()?;
    let entries = std::fs::read_dir(base_dir).ok()?;
    let mut stale = Vec::new();
    for entry in entries.flatten() {
        let user_dir = entry.path();
        let embeddings = user_dir.join("embeddings.json");
        let Ok(meta) = std::fs::metadata(&embeddings) else {
            continue;
        };
        let Ok(file_mtime) = meta.modified() else {
            continue;
        };
        if file_mtime < embedder_mtime {
            if let Some(name) = user_dir.file_name().and_then(|n| n.to_str()) {
                stale.push(name.to_owned());
            }
        }
    }
    stale.sort();
    Some(stale)
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
