// SPDX-License-Identifier: Apache-2.0
//! Windows named pipe transport for the local IPC service. The pipe DACL grants access only
//! to the current user SID and remote clients are rejected.
use crate::{
    core::Core,
    error::{Failure, Result},
    settings::Dirs,
};
use std::{ptr, sync::Arc};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
fn failed() -> std::io::Error {
    std::io::Error::other("Windows user security is unavailable.")
}
/// The current user's SID in string form.
pub fn user_sid() -> std::io::Result<String> {
    // SAFETY: Buffers stay allocated until the calls complete; the token and the SID string are
    // released on every path.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(failed());
        }
        let result = (|| {
            let mut size = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
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
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, n));
            LocalFree(sid.cast());
            Ok(text)
        })();
        CloseHandle(token);
        result
    }
}
pub fn pipe_name() -> std::io::Result<String> {
    Ok(format!(
        r"\\.\pipe\io.delino.anydash.collector.{}",
        user_sid()?
    ))
}
pub struct Listener {
    name: String,
    sddl: Vec<u16>,
    first: Option<NamedPipeServer>,
}
fn create(name: &str, sddl: &[u16], first: bool) -> std::io::Result<NamedPipeServer> {
    // SAFETY: The security descriptor is allocated by Windows, outlives the pipe creation call
    // and is released afterwards.
    unsafe {
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
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let server = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, (&raw mut attributes).cast());
        LocalFree(descriptor);
        server
    }
}
pub fn bind(_dirs: &Dirs) -> Result<Listener> {
    let name = pipe_name().map_err(|_| Failure::internal("The local IPC pipe is unavailable."))?;
    let sddl = format!(
        "D:P(A;;GA;;;{})",
        user_sid().map_err(|_| Failure::internal("The user SID is unavailable."))?
    );
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let first = create(&name, &sddl, true)
        .map_err(|_| Failure::internal("The local IPC pipe is in use."))?;
    Ok(Listener {
        name,
        sddl,
        first: Some(first),
    })
}
pub async fn serve(core: Arc<Core>, mut listener: Listener) {
    let Some(mut server) = listener.first.take() else {
        return;
    };
    loop {
        if server.connect().await.is_err() {
            continue;
        }
        let Ok(next) = create(&listener.name, &listener.sddl, false) else {
            return;
        };
        tokio::spawn(crate::ipc::serve_stream(
            Arc::clone(&core),
            std::mem::replace(&mut server, next),
        ));
    }
}
/// Detach from the console so the logon task runs without a console window.
pub fn detach_console() {
    // SAFETY: FreeConsole has no preconditions; failure leaves the process unchanged.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}
