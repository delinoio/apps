// SPDX-License-Identifier: Apache-2.0
//! Windows paths, ACLs and lifetime locks shared with the native Lane hosts.
use crate::error::{Failure, Result};
use std::{
    fs::File,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_LOCK_VIOLATION, GetLastError, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SE_FILE_OBJECT, SetNamedSecurityInfoW,
        },
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
    System::{
        Com::CoTaskMemFree,
        IO::OVERLAPPED,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
    UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
};
fn failed() -> Failure {
    Failure::new(
        "DEPENDENCY_UNAVAILABLE",
        "Windows user storage is unavailable.",
    )
}
pub fn data_root() -> Result<PathBuf> {
    // SAFETY: Windows initializes the allocated UTF-16 path; it is copied before freeing it.
    unsafe {
        let mut path = ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, ptr::null_mut(), &mut path) < 0 {
            return Err(failed());
        }
        let mut n = 0;
        while *path.add(n) != 0 {
            n += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(path, n));
        CoTaskMemFree(path.cast());
        Ok(PathBuf::from(value))
    }
}
pub fn protect(path: &Path, directory: bool) -> Result<()> {
    // SAFETY: All API buffers remain allocated until the API calls complete. Handles and allocated descriptors are released on every path.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(failed());
        }
        let result = (|| {
            let mut size = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(
                token,
                TokenUser,
                storage.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
            {
                return Err(failed());
            }
            let user = &*storage.as_ptr().cast::<TOKEN_USER>();
            let mut sid = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
                return Err(failed());
            }
            let mut n = 0;
            while *sid.add(n) != 0 {
                n += 1;
            }
            let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, n));
            LocalFree(sid.cast());
            let sddl = format!(
                "D:P(A;{};FA;;;{sid_text})",
                if directory { "OICI" } else { "" }
            );
            let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(failed());
            }
            let result = (|| {
                let mut present = 0;
                let mut defaulted = 0;
                let mut acl = ptr::null_mut();
                if GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
                    == 0
                    || present == 0
                {
                    return Err(failed());
                }
                let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                if SetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    acl,
                    ptr::null_mut(),
                ) != 0
                {
                    return Err(failed());
                }
                Ok(())
            })();
            LocalFree(descriptor);
            result
        })();
        CloseHandle(token);
        result
    }
}
pub fn try_lock(file: &File, exclusive: bool) -> std::io::Result<()> {
    // SAFETY: The file handle is live and OVERLAPPED is zeroed for byte zero. Closing the file releases this one-byte lock.
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        let flags = LOCKFILE_FAIL_IMMEDIATELY
            | if exclusive {
                LOCKFILE_EXCLUSIVE_LOCK
            } else {
                0
            };
        if LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut overlapped) != 0 {
            return Ok(());
        }
        let error = GetLastError();
        if error == ERROR_LOCK_VIOLATION {
            Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        } else {
            Err(std::io::Error::from_raw_os_error(error as i32))
        }
    }
}
