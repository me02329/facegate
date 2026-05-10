use libc::{c_char, c_int, c_void};

pub const PAM_USER: c_int = 2;
pub const PAM_SERVICE: c_int = 1;
pub const PAM_CONV: c_int = 5;

pub const PAM_TEXT_INFO: c_int = 4;

/// Opaque PAM handle — we never construct one, only forward the pointer.
#[repr(C)]
pub struct PamHandle {
    _private: [u8; 0],
}

/// A single message sent through the PAM conversation function.
#[repr(C)]
pub struct PamMessage {
    pub msg_style: c_int,
    pub msg: *const c_char,
}

/// Response slot returned by the conversation function (one per message).
#[repr(C)]
pub struct PamResponse {
    pub resp: *mut c_char,
    pub resp_retcode: c_int,
}

/// The PAM conversation structure stored in the PAM_CONV item.
#[repr(C)]
pub struct PamConv {
    /// May be null if the application provides no conversation function.
    pub conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *const *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    pub appdata_ptr: *mut c_void,
}

extern "C" {
    pub fn pam_get_item(
        pamh: *const PamHandle,
        item_type: c_int,
        item: *mut *const c_void,
    ) -> c_int;
}
