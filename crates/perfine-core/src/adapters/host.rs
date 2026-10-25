// SPDX-License-Identifier: Apache-2.0
//! Host utilities never inherit candidate, provider, or publication credentials.
use crate::types::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn executable(path: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    if path
        .extension()
        .is_some_and(|extension| !extension.eq_ignore_ascii_case("exe"))
    {
        return Err(Error::Invalid(
            "Windows host tools must be .exe files; shell wrappers are not supported",
        ));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if path.components().count() != 1 || path.to_str().is_none_or(|s| s.contains(['/', '\\', ':']))
    {
        return Err(Error::Invalid(
            "host executable must be absolute or a bare tool name",
        ));
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&search).filter(|p| p.is_absolute()) {
        let candidate = directory.join(path);
        #[cfg(windows)]
        let candidate = if candidate.extension().is_none() {
            candidate.with_extension("exe")
        } else {
            candidate
        };
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if candidate.metadata()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Ok(candidate);
        }
    }
    Err(Error::Incomplete(
        "host tool missing; install Git/Docker and add its directory to PATH, or configure an absolute Docker executable",
    ))
}

pub(crate) fn command(path: &Path) -> Result<Command> {
    let executable = executable(path)?;
    let mut command = Command::new(&executable);
    command.env_clear();
    // Only the selected utility directory is available to child utility lookups.
    // In particular, do not forward HOME, Git configuration, tokens or proxies.
    if let Some(parent) = executable.parent() {
        command.env("PATH", parent);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    Ok(command)
}

/// Docker accepts drive paths, but not Rust's canonical verbatim prefix.
/// Reject UNC/device paths and CSV delimiters instead of changing mount meaning.
pub(crate) fn mount_path(path: &Path) -> Result<String> {
    docker_path(
        path.to_str()
            .ok_or(Error::Invalid("Docker mount path UTF-8"))?,
    )
}
fn docker_path(path: &str) -> Result<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    if path.starts_with(r"\\") || path.starts_with("UNC\\") || path.contains([',', '"', '\n', '\r'])
    {
        return Err(Error::Invalid(
            "Docker mount requires a local path without commas, quotes or newlines; move the execution/auth directory",
        ));
    }
    if path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic() {
        Ok(path.replace('\\', "/"))
    } else if path.contains('\\') {
        Err(Error::Invalid("backslash in non-drive Docker mount path"))
    } else {
        Ok(path.to_owned())
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows::private_directory(path)?;
    Ok(())
}

#[cfg(windows)]
// Windows ACLs and job objects have no safe std equivalents. Keep the FFI
// boundary here; all callers use safe functions that fail on OS errors.
#[allow(unsafe_code, reason = "encapsulated Windows ACL and process job APIs")]
pub(crate) mod windows {
    use super::*;
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
        },
        System::{
            Diagnostics::ToolHelp::*,
            JobObjects::*,
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    pub(super) fn private_directory(path: &Path) -> Result<()> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Protected inheritance: only the owner and SYSTEM can access this root
        // and its children. Do not treat Windows' readonly bit as a private ACL.
        let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)\0"
            .encode_utf16()
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: Both strings are terminated and descriptor is freed after use.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let result = SetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            );
            let error = std::io::Error::last_os_error();
            LocalFree(descriptor);
            if result == 0 {
                return Err(error.into());
            }
        }
        Ok(())
    }

    pub(crate) struct Job(HANDLE);
    impl Job {
        pub(crate) fn attach(child: &std::process::Child) -> Result<Self> {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: The job owns its handle; Child keeps the process handle alive.
            unsafe {
                let handle = CreateJobObjectW(ptr::null(), ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let job = Self(handle);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                    || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                // The child starts suspended, so it cannot create descendants
                // before assignment. Resume its primary thread only after the
                // kill-on-close job has been installed successfully.
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                let mut next = Thread32First(snapshot, &mut entry);
                let mut resumed = false;
                while next != 0 {
                    if entry.th32OwnerProcessID == child.id() {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if !thread.is_null() {
                            resumed = ResumeThread(thread) != u32::MAX;
                            CloseHandle(thread);
                        }
                        break;
                    }
                    next = Thread32Next(snapshot, &mut entry);
                }
                CloseHandle(snapshot);
                if !resumed {
                    return Err(Error::Incomplete("cannot resume isolated host process"));
                }
                Ok(job)
            }
        }
        pub(crate) fn terminate(&self) {
            // SAFETY: self owns a valid job handle.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: This is the only owner of this handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub(crate) fn architecture() -> Result<&'static str> {
    // An x64 CLI can itself run under Rosetta on an ARM Mac. Probe the host
    // rather than labeling that container run as non-emulated x86_64.
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        let mut command = command(Path::new("/usr/sbin/sysctl"))?;
        command.args(["-n", "hw.optional.arm64"]);
        let output = super::process::run(
            &mut command,
            &[],
            None,
            std::time::Duration::from_secs(5),
            || {},
        )?;
        if output.interrupted || output.code != 0 {
            return Err(Error::Incomplete(
                "cannot identify Mac host CPU for emulation evidence",
            ));
        }
        match output.stdout.trim_ascii() {
            b"1" => return Ok("aarch64"),
            b"0" => return Ok("x86_64"),
            _ => return Err(Error::Incomplete("unknown Mac host CPU")),
        }
    }
    #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
    Ok(std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn private_directory_preserves_owner_access_and_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private files");
        std::fs::create_dir(&root).unwrap();
        private_directory(&root).unwrap();
        let child = root.join("child");
        std::fs::create_dir(&child).unwrap();
        let file = child.join("evidence.json");
        std::fs::write(&file, b"private evidence").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"private evidence");
        // Applying the protected ACL again must preserve existing state and the
        // owner's ability to update and delete inherited child objects.
        private_directory(&root).unwrap();
        std::fs::write(&file, b"updated evidence").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"updated evidence");
        std::fs::remove_dir_all(&root).unwrap();
        temp.close().unwrap();
    }
    #[test]
    fn docker_mount_paths_preserve_spaces_and_unicode() {
        assert_eq!(
            docker_path(r"\\?\C:\User Files\한글\build").unwrap(),
            "C:/User Files/한글/build"
        );
        assert_eq!(
            docker_path("/Users/test files/한글").unwrap(),
            "/Users/test files/한글"
        );
        for bad in [
            r"\\server\share",
            r"\\?\UNC\server\share",
            "/tmp/a,b",
            "/tmp/a\n",
        ] {
            assert!(docker_path(bad).is_err());
        }
    }
    #[test]
    fn discovery_rejects_relative_executables() {
        for path in ["./git", "../git", "tools/git", r"tools\git", "C:git"] {
            assert!(executable(Path::new(path)).is_err());
        }
        assert!(executable(Path::new("perfine-nonexistent-tool-234932")).is_err());
    }
    #[test]
    fn host_commands_do_not_inherit_credentials() {
        let cmd = command(Path::new("git")).unwrap();
        for (key, _) in cmd.get_envs() {
            assert!(matches!(
                key.to_str().unwrap(),
                "PATH" | "SystemRoot" | "WINDIR" | "TEMP" | "TMP"
            ));
        }
    }
}
