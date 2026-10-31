//! Cross-user presence marker. It is never acquired as an exclusion lock.

use std::io;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{
    CreateMutexExW, GetCurrentProcess, OpenProcessToken, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows::core::{PCWSTR, PWSTR};

const NAME: &str = r"Global\mImageViewerMcp_v1";

pub(crate) struct Marker(HANDLE);

impl Marker {
    pub(crate) fn create() -> Result<Self, String> {
        Self::named(NAME).map_err(|error| error.to_string())
    }

    fn named(name: &str) -> io::Result<Self> {
        let sid = current_user_sid()?;
        // Explicit creator SID rather than a default DACL: another user's
        // launcher and installer need SYNCHRONIZE, never MUTEX_ALL_ACCESS.
        let sddl = format!("D:(A;;0x00100000;;;WD)(A;;GA;;;{sid})");
        let sddl = wide(&sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(io::Error::other)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let name = wide(name);
        let result = unsafe {
            CreateMutexExW(
                Some(&attributes),
                PCWSTR(name.as_ptr()),
                0,
                SYNCHRONIZATION_SYNCHRONIZE.0,
            )
        }
        .map(Self)
        .map_err(io::Error::other);
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(io::Error::other)?;
    let result = (|| {
        let mut len = 0;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
        if len < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }
        .map_err(io::Error::other)?;
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(io::Error::other)?;
        let result = unsafe { sid.to_string() }.map_err(io::Error::other);
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
        }
        result
    })();
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows::Win32::Security::{ACCESS_ALLOWED_ACE, DACL_SECURITY_INFORMATION, GetAce, PSID};
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_ACCESS_RIGHTS};

    fn unique_name() -> String {
        format!(
            r"Local\mImageViewerMcp-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn marker_presence_lasts_until_last_launcher_handle_closes() {
        let name = unique_name();
        let wide_name = wide(&name);
        let first = Marker::named(&name).unwrap();
        let second = Marker::named(&name).unwrap();
        drop(first);
        let observer = unsafe {
            OpenMutexW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                PCWSTR(wide_name.as_ptr()),
            )
        }
        .unwrap();
        unsafe {
            let _ = CloseHandle(observer);
        }
        drop(second);
        let error = unsafe {
            OpenMutexW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                PCWSTR(wide_name.as_ptr()),
            )
        }
        .unwrap_err();
        assert_eq!(
            error.code(),
            windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult()
        );
    }

    #[test]
    fn actual_marker_dacl_grants_everyone_only_synchronize_and_creator_full_access() {
        let name = unique_name();
        let _marker = Marker::named(&name).unwrap();
        let name = wide(&name);
        // The normal marker handle intentionally has only SYNCHRONIZE. Its
        // creator may separately open READ_CONTROL to inspect the actual DACL.
        let observer = unsafe {
            OpenMutexW(
                SYNCHRONIZATION_ACCESS_RIGHTS(0x00020000),
                false,
                PCWSTR(name.as_ptr()),
            )
        }
        .unwrap();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let mut dacl = std::ptr::null_mut();
        let error = unsafe {
            GetSecurityInfo(
                observer,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        unsafe {
            let _ = CloseHandle(observer);
        }
        assert_eq!(error, windows::Win32::Foundation::ERROR_SUCCESS);
        assert!(!dacl.is_null());
        let mut grants = Vec::new();
        for index in 0..unsafe { (*dacl).AceCount } {
            let mut ace = std::ptr::null_mut();
            unsafe { GetAce(dacl, u32::from(index), &mut ace) }.unwrap();
            let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
            let mut sid = PWSTR::null();
            unsafe {
                ConvertSidToStringSidW(
                    PSID(std::ptr::addr_of!(ace.SidStart).cast_mut().cast()),
                    &mut sid,
                )
            }
            .unwrap();
            grants.push((unsafe { sid.to_string() }.unwrap(), ace.Mask));
            unsafe {
                let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            }
        }
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        assert_eq!(grants.len(), 2);
        assert!(grants.contains(&("S-1-1-0".to_owned(), SYNCHRONIZATION_SYNCHRONIZE.0)));
        assert!(grants.contains(&(current_user_sid().unwrap(), 0x001f0001))); // MUTEX_ALL_ACCESS
    }
}
