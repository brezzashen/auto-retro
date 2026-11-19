//! Bounded libproc reads avoid spawning lsof or ps for each hot process.
use crate::procs::Proc;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
fn info<T>(pid: i32, flavor: i32) -> Option<T> {
    let mut value = std::mem::MaybeUninit::<T>::zeroed();
    let size = std::mem::size_of::<T>();
    // SAFETY: libproc receives a correctly sized, writable buffer for this flavor.
    let read =
        unsafe { libc::proc_pidinfo(pid, flavor, 0, value.as_mut_ptr().cast(), size as i32) };
    if read as usize != size {
        return None;
    }
    // SAFETY: libproc initialized the complete buffer on success.
    Some(unsafe { value.assume_init() })
}

pub fn same(p: &Proc) -> bool {
    #[cfg(target_os = "macos")]
    {
        info::<libc::proc_bsdinfo>(p.pid, libc::PROC_PIDTBSDINFO)
            .is_some_and(|i| i.pbi_uid == p.uid && (i.pbi_start_tvsec as i64 - p.start).abs() <= 2)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = p;
        false
    }
}

pub fn paths(pid: i32) -> (Option<PathBuf>, Option<PathBuf>) {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0u8; 4096];
        // SAFETY: the output buffer is writable and its actual capacity is supplied.
        let count =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        let executable = (count > 0).then(|| path_bytes(&buffer)).flatten();
        let cwd =
            info::<libc::proc_vnodepathinfo>(pid, libc::PROC_PIDVNODEPATHINFO).and_then(|v| {
                path_bytes(
                    &v.pvi_cdir
                        .vip_path
                        .iter()
                        .flatten()
                        .map(|v| *v as u8)
                        .collect::<Vec<_>>(),
                )
            });
        (executable, cwd)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        (None, None)
    }
}

#[cfg(target_os = "macos")]
fn path_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let end = bytes.iter().position(|v| *v == 0).unwrap_or(bytes.len());
    (end > 0).then(|| std::ffi::OsString::from_vec(bytes[..end].to_vec()).into())
}
