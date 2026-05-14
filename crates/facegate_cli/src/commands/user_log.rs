use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{bail, Context as _};

const LOG_DIR: &str = ".local/state/facegate";
const LOG_FILE: &str = "facegate.log";

#[derive(Debug, Clone)]
struct UserInfo {
    uid: u32,
    gid: u32,
    home: PathBuf,
}

pub fn run(lines: usize) -> anyhow::Result<()> {
    let user = current_user()?;
    let path = log_path(&user);
    println!("{}", path.display());
    println!();

    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("No Facegate user log yet.");
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };

    let reader = std::io::BufReader::new(file);
    let mut all = reader
        .lines()
        .map_while(std::result::Result::ok)
        .collect::<Vec<_>>();
    let keep_from = all.len().saturating_sub(lines.max(1));
    for line in all.drain(keep_from..) {
        println!("{}", display_line(&line));
    }
    Ok(())
}

pub fn run_streaming(lines: usize, tx: &std::sync::mpsc::Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let user = current_user()?;
    let path = log_path(&user);
    out!("{}", path.display());
    out!("");

    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out!("No Facegate user log yet.");
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };

    let reader = std::io::BufReader::new(file);
    let mut all = reader
        .lines()
        .map_while(std::result::Result::ok)
        .collect::<Vec<_>>();
    let keep_from = all.len().saturating_sub(lines.max(1));
    for line in all.drain(keep_from..) {
        out!("{}", display_line(&line));
    }
    Ok(())
}

pub fn append_for_user(username: &str, message: impl AsRef<str>) {
    let Ok(user) = user_by_name(username) else {
        return;
    };
    append(&user, message.as_ref());
}

pub fn append_for_current_or_sudo_user(message: impl AsRef<str>) {
    let Ok(user) = current_user() else {
        return;
    };
    append(&user, message.as_ref());
}

fn append(user: &UserInfo, message: &str) {
    let path = log_path(user);
    let Some(parent) = path.parent() else {
        return;
    };
    if prepare_log_dir(parent, user).is_err() {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    fix_owner_if_root(&path, user);
    let sanitized = message.replace('\n', " ");
    let _ = writeln!(
        file,
        "{} | {}",
        format_unix_utc(unix_now() as i64),
        render_message(&sanitized)
    );
}

fn display_line(line: &str) -> String {
    if let Some((stamp, message)) = line.split_once(" | ") {
        return format!("{stamp} | {}", render_message(message));
    }

    let Some((epoch, message)) = line.split_once(' ') else {
        return render_message(line);
    };
    let Ok(epoch) = epoch.parse::<i64>() else {
        return render_message(line);
    };
    format!("{} | {}", format_unix_utc(epoch), render_message(message))
}

fn render_message(message: &str) -> String {
    let message = message.trim();
    let Some((event, rest)) = split_event(message) else {
        return message.to_owned();
    };
    let fields = parse_fields(rest);

    match event {
        "test start" => {
            let scope = field(&fields, "scope").unwrap_or("unknown");
            let enrolled = field(&fields, "enrolled").unwrap_or("?");
            format!("test: started (scope {scope}, {enrolled} enrolled templates)")
        }
        "test captured" => match rest {
            "rgb_ir_probe" => "test: captured synchronized RGB+IR probe".to_owned(),
            "single_camera_probe" => "test: captured single-camera probe".to_owned(),
            _ => format!("test: captured {rest}"),
        },
        "test reject" => {
            let reason = field(&fields, "reason").unwrap_or("unknown");
            format!("test: rejected ({})", human_reason(reason))
        }
        "test retry" => render_retry("test", &fields),
        "test result" => {
            let result = field(&fields, "result")
                .unwrap_or("unknown")
                .to_ascii_uppercase();
            let reason = field(&fields, "reason").map(human_reason);
            let score = field(&fields, "score").unwrap_or("?");
            let threshold = field(&fields, "threshold").unwrap_or("?");
            match reason {
                Some(reason) => {
                    format!("test: {result} (score {score}, threshold {threshold}, {reason})")
                }
                None => format!("test: {result} (score {score}, threshold {threshold})"),
            }
        }
        "auth start" => {
            let service = field(&fields, "service").unwrap_or("unknown");
            format!("auth: started for service {service}")
        }
        "auth accept" => render_decision("auth", "accepted", &fields),
        "auth reject" => render_attempt_decision("auth", "rejected", &fields),
        "auth retry" => render_retry("auth", &fields),
        "auth timeout" => render_attempt_device("auth", "timeout", &fields),
        "auth camera_error" => render_device_error("auth", "camera error", &fields),
        "auth capture_error" => render_attempt_device_error("auth", "capture error", &fields),
        "auth final=timeout" => "auth: final timeout".to_owned(),
        "auth final=not_recognized" => "auth: final not recognized".to_owned(),
        "auth broker_unavailable" => {
            let code = field(&fields, "code").unwrap_or("unknown");
            format!("auth: broker unavailable ({code})")
        }
        "auth broker_error" => {
            let error = field(&fields, "error").unwrap_or("unknown");
            format!("auth: broker error ({error})")
        }
        "watch scan_start" => "watch: scan started".to_owned(),
        "watch accept" => render_decision("watch", "accepted", &fields),
        "watch reject" => render_attempt_decision("watch", "rejected", &fields),
        "watch retry" => render_retry("watch", &fields),
        "watch timeout" => render_attempt_device("watch", "timeout", &fields),
        "watch camera_error" => render_device_error("watch", "camera error", &fields),
        "watch capture_error" => render_attempt_device_error("watch", "capture error", &fields),
        "watch exhausted_attempts" => "watch: exhausted attempts".to_owned(),
        "watch broker_error" => {
            let error = field(&fields, "error").unwrap_or("unknown");
            format!("watch: broker error ({error})")
        }
        "services refresh" => {
            let broker = field(&fields, "broker").unwrap_or("unknown");
            let watch = field(&fields, "watch").unwrap_or("unknown");
            format!("services: refreshed (broker {broker}, watch {watch})")
        }
        "services refresh_error" => {
            let error = field(&fields, "error").unwrap_or("unknown");
            format!("services: refresh error ({error})")
        }
        _ => message.to_owned(),
    }
}

fn split_event(message: &str) -> Option<(&str, &str)> {
    const EVENTS: &[&str] = &[
        "test start",
        "test captured",
        "test reject",
        "test retry",
        "test result",
        "auth start",
        "auth accept",
        "auth reject",
        "auth retry",
        "auth timeout",
        "auth camera_error",
        "auth capture_error",
        "auth final=timeout",
        "auth final=not_recognized",
        "auth broker_unavailable",
        "auth broker_error",
        "watch scan_start",
        "watch accept",
        "watch reject",
        "watch retry",
        "watch timeout",
        "watch camera_error",
        "watch capture_error",
        "watch exhausted_attempts",
        "watch broker_error",
        "services refresh_error",
        "services refresh",
    ];
    EVENTS.iter().find_map(|event| {
        message
            .strip_prefix(event)
            .map(|rest| (*event, rest.trim_start()))
    })
}

fn parse_fields(input: &str) -> Vec<(&str, &str)> {
    input
        .split_whitespace()
        .filter_map(|part| part.split_once('='))
        .collect()
}

fn field<'a>(fields: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|(key, value)| (*key == name).then_some(*value))
}

fn render_decision(kind: &str, decision: &str, fields: &[(&str, &str)]) -> String {
    let score = field(fields, "score").unwrap_or("?");
    let reason = field(fields, "reason").map(human_reason);
    match reason {
        Some(reason) => format!("{kind}: {decision} (score {score}, {reason})"),
        None => format!("{kind}: {decision} (score {score})"),
    }
}

fn render_attempt_decision(kind: &str, decision: &str, fields: &[(&str, &str)]) -> String {
    let attempt = field(fields, "attempt").unwrap_or("?");
    let score = field(fields, "score").unwrap_or("?");
    let reason = field(fields, "reason").map(human_reason);
    match reason {
        Some(reason) => {
            format!("{kind}: attempt {attempt} {decision} (score {score}, {reason})")
        }
        None => format!("{kind}: attempt {attempt} {decision} (score {score})"),
    }
}

fn render_retry(kind: &str, fields: &[(&str, &str)]) -> String {
    let attempt = field(fields, "attempt").unwrap_or("?");
    let capture_attempt = field(fields, "capture_attempt").unwrap_or(attempt);
    let reason = field(fields, "reason")
        .map(human_reason)
        .unwrap_or("unknown reason");
    format!("{kind}: retrying capture {capture_attempt} ({reason})")
}

fn render_attempt_device(kind: &str, action: &str, fields: &[(&str, &str)]) -> String {
    let attempt = field(fields, "attempt").unwrap_or("?");
    let device = field(fields, "device").unwrap_or("unknown");
    format!("{kind}: attempt {attempt} {action} on {device}")
}

fn render_device_error(kind: &str, action: &str, fields: &[(&str, &str)]) -> String {
    let device = field(fields, "device").unwrap_or("unknown");
    let error = field(fields, "error").unwrap_or("unknown");
    format!("{kind}: {action} on {device} ({error})")
}

fn render_attempt_device_error(kind: &str, action: &str, fields: &[(&str, &str)]) -> String {
    let attempt = field(fields, "attempt").unwrap_or("?");
    let device = field(fields, "device").unwrap_or("unknown");
    let error = field(fields, "error").unwrap_or("unknown");
    format!("{kind}: attempt {attempt} {action} on {device} ({error})")
}

fn human_reason(reason: &str) -> &'static str {
    match reason {
        "matched" => "matched",
        "template_mismatch" => "best template score is below threshold",
        "not_enrolled" => "no enrolled template",
        "no_face" | "no_score" => "no face detected",
        "multiple_faces" => "multiple faces detected",
        "cross_check_required" => "RGB+IR cross-check is required",
        "cross_check_time_skew" => "RGB and IR frames were not synchronized",
        "cross_check_rgb_no_face" => "no face detected on RGB frame",
        "cross_check_rgb_multiple_faces" => "multiple faces detected on RGB frame",
        "cross_check_ir_no_face" => "no face detected on IR frame",
        "cross_check_ir_multiple_faces" => "multiple faces detected on IR frame",
        "cross_check_position_mismatch" => "RGB/IR face positions do not align",
        "cross_check_identity_mismatch" => "RGB/IR identities do not match",
        "cross_check_no_score" => "old log: cross-check rejected before template comparison",
        "internal" => "internal error",
        _ => "unknown reason",
    }
}

fn prepare_log_dir(path: &std::path::Path, user: &UserInfo) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    fix_owner_if_root(path, user);
    Ok(())
}

fn fix_owner_if_root(path: &std::path::Path, user: &UserInfo) {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    unsafe {
        libc::chown(c_path.as_ptr(), user.uid, user.gid);
    }
}

fn log_path(user: &UserInfo) -> PathBuf {
    user.home.join(LOG_DIR).join(LOG_FILE)
}

fn current_user() -> anyhow::Result<UserInfo> {
    let name = std::env::var("SUDO_USER")
        .ok()
        .filter(|name| !name.is_empty() && name != "root")
        .or_else(|| std::env::var("USER").ok().filter(|name| !name.is_empty()))
        .ok_or_else(|| anyhow::anyhow!("cannot determine current user"))?;
    let user = user_by_name(&name)?;
    if unsafe { libc::geteuid() } != 0 && user.uid != unsafe { libc::geteuid() } {
        bail!("refusing to read another user's Facegate log");
    }
    Ok(user)
}

fn user_by_name(username: &str) -> anyhow::Result<UserInfo> {
    let c_name = std::ffi::CString::new(username)
        .with_context(|| format!("invalid username '{username}'"))?;
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
        return Err(std::io::Error::from_raw_os_error(rc)).context("getpwnam_r failed");
    }
    if result.is_null() {
        bail!("unknown user '{username}'");
    }

    let home = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    Ok(UserInfo {
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
        home: PathBuf::from(home),
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn format_unix_utc(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let seconds_of_day = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::{display_line, format_unix_utc, render_message};

    #[test]
    fn formats_epoch_as_utc_timestamp() {
        assert_eq!(format_unix_utc(1_704_067_200), "2024-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(1_709_210_096), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn displays_legacy_epoch_logs_as_readable_lines() {
        assert_eq!(
            display_line("1778616292 test reject reason=cross_check_no_score"),
            "2026-05-12T20:04:52Z | test: rejected (old log: cross-check rejected before template comparison)"
        );
    }

    #[test]
    fn renders_match_reasons() {
        assert_eq!(
            render_message("test reject reason=cross_check_ir_no_face"),
            "test: rejected (no face detected on IR frame)"
        );
    }
}
