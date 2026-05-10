//! PAM module for facial authentication.
//!
//! This module delegates all heavy work to `facegate auth --user <name>`,
//! keeping the PAM module small, auditable, and free of ML dependencies.
//!
//! Expected PAM config line:
//!   auth sufficient pam_facegate.so

mod pam_sys;

use libc::{c_char, c_int, c_void};
use pam_sys::{
    pam_get_item, PamConv, PamHandle, PamMessage, PAM_CONV, PAM_SERVICE, PAM_TEXT_INFO, PAM_USER,
};
use std::ffi::{CStr, CString};
use std::process::Command;
use std::time::Duration;

// PAM return codes
const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_IGNORE: c_int = 25;

/// facegate auth exit codes (must match facegate_core::error::AuthExitCode)
const EXIT_RECOGNIZED: c_int = 0;
const EXIT_NOT_RECOGNIZED: c_int = 1;
const EXIT_DENIED: c_int = 6;

/// Default helper binary path (overridable via `pam_facegate.so path=...`).
const DEFAULT_HELPER_PATH: &str = "/usr/bin/facegate";

/// Hard timeout for the helper process. Must be >= max_attempts × camera.timeout_ms
/// in the facegate config plus a small slack for model load. With the defaults
/// (3 attempts × 5 s + ~3 s init) 25 s is comfortable; 45 s was unnecessarily
/// long and made password fallback feel sluggish on a missed face.
const HELPER_TIMEOUT_SECS: u64 = 25;

/// `pam_sm_authenticate` — called by PAM to authenticate the user.
///
/// # Safety
/// Called by the PAM runtime; pamh and argc/argv are owned by PAM.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut PamHandle,
    _flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    let username = match get_username(pamh) {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };
    let service = get_pam_item_string(pamh, PAM_SERVICE);
    let args = parse_args(argc, argv);

    send_info(pamh, "[ facegate ] Scanning face\u{2026}");

    let helper = args.helper_path.as_deref().unwrap_or(DEFAULT_HELPER_PATH);
    match run_auth_helper(helper, &username, service.as_deref()) {
        Ok(EXIT_RECOGNIZED) => PAM_SUCCESS,
        Ok(EXIT_NOT_RECOGNIZED) => PAM_IGNORE,
        Ok(EXIT_DENIED) => PAM_AUTH_ERR,
        Ok(_) => PAM_AUTH_ERR,
        Err(_) => PAM_IGNORE, // helper failed to run → fall through to next PAM module
    }
}

#[derive(Default)]
struct ModuleArgs {
    helper_path: Option<String>,
}

/// Parse `key=value` PAM module arguments. Currently recognised:
///   - `path=/abs/path/to/facegate` — override the helper binary path
unsafe fn parse_args(argc: c_int, argv: *const *const c_char) -> ModuleArgs {
    let mut out = ModuleArgs::default();
    if argv.is_null() || argc <= 0 {
        return out;
    }
    for i in 0..argc as isize {
        let raw = *argv.offset(i);
        if raw.is_null() {
            continue;
        }
        let Ok(s) = CStr::from_ptr(raw).to_str() else {
            continue;
        };
        if let Some(p) = s.strip_prefix("path=") {
            if !p.is_empty() {
                out.helper_path = Some(p.to_owned());
            }
        }
    }
    out
}

/// `pam_sm_setcred` — required symbol even if unused.
///
/// # Safety
/// Called by the PAM runtime.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_setcred(
    _pamh: *mut PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}

/// Send a PAM_TEXT_INFO message through the application's conversation function.
/// Silently ignored if the conversation is unavailable.
unsafe fn send_info(pamh: *mut PamHandle, text: &str) {
    let mut item: *const c_void = std::ptr::null();
    if pam_get_item(pamh, PAM_CONV, &mut item) != PAM_SUCCESS || item.is_null() {
        return;
    }
    let conv = &*(item as *const PamConv);
    let Some(conv_fn) = conv.conv else { return };
    let Ok(c_text) = CString::new(text) else {
        return;
    };
    let msg = PamMessage {
        msg_style: PAM_TEXT_INFO,
        msg: c_text.as_ptr(),
    };
    let msg_ptr: *const PamMessage = &msg;
    let mut resp: *mut pam_sys::PamResponse = std::ptr::null_mut();
    let n_msg: c_int = 1;
    conv_fn(n_msg, &msg_ptr, &mut resp, conv.appdata_ptr);
    // PAM spec: the conv may allocate a response array of `num_msg` entries;
    // each entry's `resp` string is also heap-allocated. The module must free
    // both (resp[i].resp) and the array itself.
    if !resp.is_null() {
        for i in 0..n_msg as isize {
            let entry = resp.offset(i);
            let s = (*entry).resp;
            if !s.is_null() {
                libc::free(s as *mut c_void);
            }
        }
        libc::free(resp as *mut c_void);
    }
}

/// Retrieve the PAM username as an owned String.
unsafe fn get_username(pamh: *mut PamHandle) -> Option<String> {
    get_pam_item_string(pamh, PAM_USER)
}

unsafe fn get_pam_item_string(pamh: *mut PamHandle, item_type: c_int) -> Option<String> {
    let mut item: *const c_void = std::ptr::null();
    let ret = pam_get_item(pamh, item_type, &mut item);
    if ret != PAM_SUCCESS || item.is_null() {
        return None;
    }
    let cstr = CStr::from_ptr(item as *const c_char);
    cstr.to_str().ok().map(|s| s.to_owned())
}

fn run_auth_helper(helper: &str, username: &str, service: Option<&str>) -> Result<c_int, ()> {
    let mut command = Command::new(helper);
    command.args(["auth", "--user", username]);
    if let Some(service) = service {
        command.args(["--service", service]);
    }
    let mut child = command.spawn().map_err(|_| ())?;

    // Wait with a hard timeout so PAM is never blocked indefinitely.
    let deadline = std::time::Instant::now() + Duration::from_secs(HELPER_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(status.code().unwrap_or(-1));
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(()),
        }
    }
}
