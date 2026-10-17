//! hyprwin IPC over named pipes, restricted to the current user.
//!
//! - Request pipe (`\\.\pipe\hyprwin-<user>`): the client writes one request
//!   (`[j/]command args`, like Hyprland's socket1), the server writes the reply
//!   and closes. `--batch` requests are `[[BATCH]]cmd1;cmd2`.
//! - Event pipe (`\\.\pipe\hyprwin-events-<user>`): the server streams
//!   `EVENT>>DATA\n` lines, like Hyprland's socket2.

use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

pub const BATCH_PREFIX: &str = "[[BATCH]]";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn user_tag() -> String {
    let u = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    u.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

pub fn request_pipe_name() -> String {
    format!(r"\\.\pipe\hyprwin-{}", user_tag())
}

pub fn event_pipe_name() -> String {
    format!(r"\\.\pipe\hyprwin-events-{}", user_tag())
}

/// SDDL granting access only to the current user and SYSTEM.
fn current_user_sddl() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let token = OwnedHandle::from_raw_handle(token.0 as _);
        let mut len = 0u32;
        let _ = GetTokenInformation(HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&token) as _), TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        GetTokenInformation(
            HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&token) as _),
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        )
        .ok()?;
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(tu.User.Sid, &mut s).ok()?;
        let sid = s.to_string().ok()?;
        let _ = LocalFree(Some(HLOCAL(s.0 as *mut c_void)));
        Some(format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"))
    }
}

/// Owns a security descriptor for pipe creation.
pub struct PipeSecurity {
    sd: PSECURITY_DESCRIPTOR,
}

impl PipeSecurity {
    pub fn current_user() -> Option<Self> {
        let sddl = wide(&current_user_sddl()?);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None).ok()? };
        Some(Self { sd })
    }
    fn attrs(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.sd.0, bInheritHandle: false.into() }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.sd.0)));
        }
    }
}

unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

/// Creates one server instance and blocks until a client connects.
/// Returns the connected pipe as a `File`.
pub fn accept(name: &str, sec: &PipeSecurity, outbound_only: bool) -> std::io::Result<std::fs::File> {
    let n = wide(name);
    let sa = sec.attrs();
    let access = if outbound_only { PIPE_ACCESS_OUTBOUND } else { PIPE_ACCESS_DUPLEX };
    let h = unsafe {
        CreateNamedPipeW(
            PCWSTR(n.as_ptr()),
            access,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(&sa),
        )
    };
    if h.is_invalid() {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_handle(h.0 as _) };
    // ERROR_PIPE_CONNECTED (client connected between create and connect) is success.
    if let Err(e) = unsafe { ConnectNamedPipe(h, None) } {
        if e.code().0 as u32 != 0x8007_0217 {
            return Err(std::io::Error::other(e.to_string()));
        }
    }
    Ok(file)
}

/// Connects to a pipe as a client, waiting up to `timeout_ms` for a free instance.
pub fn connect(name: &str, write: bool, timeout_ms: u32) -> std::io::Result<std::fs::File> {
    use std::time::{Duration, Instant};
    let n = wide(name);
    let access = if write { GENERIC_READ.0 | GENERIC_WRITE.0 } else { GENERIC_READ.0 };
    let start = Instant::now();
    let deadline = start + Duration::from_millis(timeout_ms as u64);
    // A missing pipe usually means no daemon, so give up on that quickly
    // (the server re-listens within microseconds; callers such as the
    // Settings window poll on their UI thread).
    let not_found_deadline = start + Duration::from_millis(timeout_ms.min(50) as u64);
    loop {
        let h = unsafe { CreateFileW(PCWSTR(n.as_ptr()), access, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) };
        let now = Instant::now();
        match h {
            Ok(h) => return Ok(unsafe { std::fs::File::from_raw_handle(h.0 as _) }),
            // ERROR_PIPE_BUSY: every instance is taken; wait for one.
            Err(e) if e.code().0 as u32 == 0x8007_00E7 && now < deadline => {
                let left = (deadline - now).as_millis().max(1) as u32;
                let _ = unsafe { WaitNamedPipeW(PCWSTR(n.as_ptr()), left) };
            }
            // ERROR_FILE_NOT_FOUND: no daemon, or it is between two pipe
            // instances for a moment. Retry briefly.
            Err(e) if e.code().0 as u32 == 0x8007_0002 && now < not_found_deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(std::io::Error::other(format!("{name}: {e}"))),
        }
    }
}

/// Client helper: sends one request and returns the full reply.
pub fn request(req: &str) -> std::io::Result<String> {
    let mut f = connect(&request_pipe_name(), true, 2000)?;
    // Length-prefixed request so the server knows where it ends without closing.
    let bytes = req.as_bytes();
    f.write_all(&(bytes.len() as u32).to_le_bytes())?;
    f.write_all(bytes)?;
    let mut out = String::new();
    f.read_to_string(&mut out)?;
    Ok(out)
}

/// Server helper: reads one length-prefixed request.
pub fn read_request(f: &mut std::fs::File) -> std::io::Result<String> {
    let mut len = [0u8; 4];
    f.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 1 << 20 {
        return Err(std::io::Error::other("request too large"));
    }
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| std::io::Error::other("request is not UTF-8"))
}
