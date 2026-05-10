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

/// Timeout for the helper process.
const HELPER_TIMEOUT_SECS: u64 = 45;

/// `pam_sm_authenticate` — called by PAM to authenticate the user.
///
/// # Safety
/// Called by the PAM runtime; pamh and argc/argv are owned by PAM.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    let username = match get_username(pamh) {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };
    let service = get_pam_item_string(pamh, PAM_SERVICE);

    send_info(pamh, "Scanning face\u{2026}");

    match run_auth_helper(&username, service.as_deref()) {
        Ok(EXIT_RECOGNIZED) => PAM_SUCCESS,
        Ok(EXIT_NOT_RECOGNIZED) => PAM_IGNORE,
        Ok(EXIT_DENIED) => PAM_AUTH_ERR,
        Ok(_) => PAM_AUTH_ERR,
        Err(_) => PAM_IGNORE, // helper failed to run → fall through to next PAM module
    }
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
    conv_fn(1, &msg_ptr, &mut resp, conv.appdata_ptr);
    // Free any response the application allocated (PAM spec requires the module to free it).
    if !resp.is_null() {
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

fn run_auth_helper(username: &str, service: Option<&str>) -> Result<c_int, ()> {
    let mut command = Command::new("/usr/bin/facegate");
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
