//! Windows has no public default-endpoint setter. Keep the PolicyConfig ABI in
//! this compatibility boundary; unsupported systems fail the activation probe.
//! ABI reference: https://github.com/amate/SetDefaultAudioDevice/blob/master/PolicyConfig.h
use windows::core::{IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Media::Audio::ERole;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

#[repr(transparent)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyConfig(IUnknown);

// This COM IID always identifies the vtable below; IUnknown owns the reference.
unsafe impl Interface for PolicyConfig {
    type Vtable = PolicyConfigVtable;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}

#[repr(C)]
pub struct PolicyConfigVtable {
    base: IUnknown_Vtbl,
    // GetMixFormat through SetPropertyValue, which Monarch never calls.
    unused: [usize; 10],
    set_default_endpoint:
        unsafe extern "system" fn(*mut std::ffi::c_void, PCWSTR, ERole) -> HRESULT,
}

impl PolicyConfig {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            CoCreateInstance(
                &GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9),
                None,
                CLSCTX_ALL,
            )
        }
    }

    pub fn set_default(&self, id: &str, role: ERole) -> windows::core::Result<()> {
        let id: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        // The queried IID fixes this vtable layout. The UTF-16 buffer outlives
        // the synchronous call, and callers validate it contains no embedded NUL.
        unsafe {
            (self.vtable().set_default_endpoint)(self.as_raw(), PCWSTR(id.as_ptr()), role).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setter_follows_iunknown_and_ten_policy_methods() {
        assert_eq!(
            std::mem::offset_of!(PolicyConfigVtable, set_default_endpoint),
            13 * std::mem::size_of::<usize>()
        );
    }
}
