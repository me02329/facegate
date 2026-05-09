//! PAM module for facial authentication.
//!
//! This module delegates all heavy work to `facegate auth --user <name>`,
//! keeping the PAM module small, auditable, and free of ML dependencies.
//!
//! Expected PAM config line:
//!   auth sufficient pam_facegate.so

mod pam_sys;

use libc::{c_char, c_int, c_void};
use pam_sys::{pam_get_item, PamHandle, PAM_USER};
use std::ffi::CStr;
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
const HELPER_TIMEOUT_SECS: u64 = 10;

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

    match run_auth_helper(&username) {
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

/// Retrieve the PAM username as an owned String.
unsafe fn get_username(pamh: *mut PamHandle) -> Option<String> {
    let mut item: *const c_void = std::ptr::null();
    let ret = pam_get_item(pamh, PAM_USER, &mut item);
    if ret != PAM_SUCCESS || item.is_null() {
        return None;
    }
    let cstr = CStr::from_ptr(item as *const c_char);
    cstr.to_str().ok().map(|s| s.to_owned())
}

fn run_auth_helper(username: &str) -> Result<c_int, ()> {
    let mut child = Command::new("/usr/bin/facegate")
        .args(["auth", "--user", username])
        .spawn()
        .map_err(|_| ())?;

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
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(()),
        }
    }
}
