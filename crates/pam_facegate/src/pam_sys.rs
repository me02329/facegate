use libc::{c_int, c_void};

pub const PAM_USER: c_int = 2;
pub const PAM_SERVICE: c_int = 1;

/// Opaque PAM handle — we never construct one, only forward the pointer.
#[repr(C)]
pub struct PamHandle {
    _private: [u8; 0],
}

extern "C" {
    pub fn pam_get_item(
        pamh: *const PamHandle,
        item_type: c_int,
        item: *mut *const c_void,
    ) -> c_int;
}

/// Item type enum (only what we use).
#[allow(dead_code)]
pub enum PamItem {
    User = 2,
}
