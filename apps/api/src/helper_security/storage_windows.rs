use super::*;
use std::{
    ffi::c_void,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
};
#[cfg(test)]
#[path = "storage_windows_tests.rs"]
mod tests;
type Handle = *mut c_void;
#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: Handle,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(handle: isize) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn LocalFree(pointer: Handle) -> Handle;
}
#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        buffer: Handle,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    fn GetSecurityInfo(
        handle: Handle,
        kind: u32,
        information: u32,
        owner: *mut Handle,
        group: Handle,
        dacl: Handle,
        sacl: Handle,
        descriptor: *mut Handle,
    ) -> u32;
    fn EqualSid(first: Handle, second: Handle) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        s: *const u16,
        revision: u32,
        descriptor: *mut Handle,
        size: *mut u32,
    ) -> i32;
    fn GetSecurityDescriptorDacl(
        descriptor: Handle,
        present: *mut i32,
        dacl: *mut Handle,
        defaulted: *mut i32,
    ) -> i32;
    fn SetSecurityInfo(
        handle: Handle,
        kind: u32,
        information: u32,
        owner: Handle,
        group: Handle,
        dacl: Handle,
        sacl: Handle,
    ) -> u32;
}
struct Owned(Handle);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as isize);
        }
    }
}
struct Descriptor(Handle);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) fn protect(root: &Path) -> io::Result<()> {
    let path: Vec<_> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    let null = std::ptr::null_mut();
    // Hold the directory against replacement/deletion while checking ownership
    // and setting its protected DACL. Never follow a final reparse point.
    unsafe {
        let handle = CreateFileW(
            path.as_ptr(),
            0x20000 | 0x40000,
            1 | 2,
            null,
            3,
            0x02000000 | 0x00200000,
            null,
        );
        if handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let directory = std::fs::File::from_raw_handle(handle);
        let metadata = directory.metadata()?;
        if is_redirect(&metadata) || !metadata.is_dir() {
            return Err(denied());
        }
        let handle = directory.as_raw_handle();
        let mut token = null;
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Owned(token);
        // TOKEN_USER begins with an aligned SID_AND_ATTRIBUTES. Fixed storage
        // exceeds the maximum Windows SID size; failure is closed.
        let mut user = [0usize; 64];
        let mut needed = 0;
        if GetTokenInformation(
            token.0,
            1,
            user.as_mut_ptr().cast(),
            std::mem::size_of_val(&user) as u32,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut owner = null;
        let mut descriptor = null;
        let status = GetSecurityInfo(handle, 1, 1, &mut owner, null, null, null, &mut descriptor);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _owner_descriptor = Descriptor(descriptor);
        if EqualSid(owner, user[0] as Handle) == 0 {
            return Err(denied());
        }
        // Owner Rights, full access, inherited by children; no Users/Everyone
        // entry and no inherited grants from a public custom parent.
        let s: Vec<_> = "D:P(A;OICI;FA;;;OW)"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null;
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            s.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let descriptor = Descriptor(descriptor);
        let (mut present, mut defaulted, mut dacl) = (0, 0, null);
        if GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) == 0
            || present == 0
            || dacl.is_null()
        {
            return Err(denied());
        }
        let status = SetSecurityInfo(handle, 1, 0x4 | 0x80000000, null, null, dacl, null);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    Ok(())
}
