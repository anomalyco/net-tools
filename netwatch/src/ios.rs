//! Safe owner of a Network.framework path subscription. `path_monitor.c`
//! confines SDK object and block lifetimes to the native side of the boundary.
use std::{ffi::c_void, net::SocketAddr, ptr::NonNull, sync::mpsc, time::Duration};

use socket2::{SockAddr, SockAddrStorage};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Path {
    pub(crate) interface_index: u32,
    pub(crate) gateway: Option<SocketAddr>,
}

unsafe extern "C" {
    fn netwatch_path_start(
        context: *mut c_void,
        update: unsafe extern "C" fn(*mut c_void, u32, *const libc::sockaddr),
        cancelled: unsafe extern "C" fn(*mut c_void),
    ) -> *mut c_void;
    fn netwatch_path_stop(monitor: *mut c_void);
}

#[derive(Debug)]
pub(crate) struct PathMonitor(NonNull<c_void>);

// SAFETY: the only operation on this owned handle after construction is
// cancel/release. Neither has queue affinity. Callbacks never borrow the handle.
unsafe impl Send for PathMonitor {}

impl PathMonitor {
    fn start<F: FnMut(Path) + Send + 'static>(f: F) -> Option<Self> {
        unsafe extern "C" fn update<F: FnMut(Path)>(
            ctx: *mut c_void,
            index: u32,
            gateway: *const libc::sockaddr,
        ) {
            // SAFETY: the shim passes NULL or a live, aligned IPv4/IPv6 sockaddr
            // until this callback returns. Copy it; do not retain the pointer.
            let gateway = unsafe { socket_addr(gateway, None) };
            // SAFETY: updates are serialized on a private dispatch queue, and
            // the cancel handler can free ctx only after the last update returns.
            let callback = unsafe { &mut *ctx.cast::<F>() };
            callback(Path {
                interface_index: index,
                gateway,
            });
        }
        unsafe extern "C" fn cancelled<F>(ctx: *mut c_void) {
            // SAFETY: called exactly once after the last update on success.
            drop(unsafe { Box::from_raw(ctx.cast::<F>()) });
        }

        let ctx = Box::into_raw(Box::new(f)).cast();
        // SAFETY: callback signatures match path_monitor.c. The allocation is
        // transferred only on success and is independent of this Rust handle.
        let monitor = unsafe { netwatch_path_start(ctx, update::<F>, cancelled::<F>) };
        if let Some(monitor) = NonNull::new(monitor) {
            Some(Self(monitor))
        } else {
            // SAFETY: on failure the shim schedules neither callback.
            unsafe { cancelled::<F>(ctx) };
            None
        }
    }

    pub(crate) fn subscribe(
        mut notify: impl FnMut() + Send + 'static,
    ) -> Option<(Self, watch::Receiver<Option<Path>>)> {
        let (tx, rx) = watch::channel(None);
        let monitor = Self::start(move |path| {
            tx.send_replace(Some(path));
            notify();
        })?;
        Some((monitor, rx))
    }
}

impl Drop for PathMonitor {
    fn drop(&mut self) {
        // SAFETY: this is our one owned create reference. Cancellation may
        // finish later; the native cancel handler, not Drop, destroys the box.
        unsafe { netwatch_path_stop(self.0.as_ptr()) };
    }
}

impl Path {
    pub(crate) async fn latest(rx: &mut watch::Receiver<Option<Self>>) -> Option<Self> {
        rx.wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|path| *path)
    }

    pub(crate) async fn current() -> Option<Self> {
        let (_monitor, mut rx) = PathMonitor::subscribe(|| {})?;
        Self::latest(&mut rx).await
    }

    /// HomeRouter is a synchronous, best-effort API. Bound how long its caller
    /// can be held waiting for the first OS callback; the queue is not main.
    pub(crate) fn current_blocking() -> Option<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let _monitor = PathMonitor::start(move |p| {
            let _ = tx.try_send(p);
        })?;
        rx.recv_timeout(Duration::from_secs(1)).ok()
    }
}

/// Read an IP sockaddr from the iOS SDK. Darwin may omit trailing zeros
/// (and the family) from a netmask; the interface address supplies its family.
///
/// # Safety
/// `raw` is null or points to an OS sockaddr valid for its `sa_len` bytes,
/// including at least the length byte even when `sa_len` is zero.
pub(crate) unsafe fn socket_addr(
    raw: *const libc::sockaddr,
    mask_family: Option<i32>,
) -> Option<SocketAddr> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: only read the length until we know how many bytes are present.
    let len = unsafe { usize::from((*raw).sa_len) };
    let family = match mask_family {
        Some(f) => f,
        None if len >= 2 => unsafe { i32::from((*raw).sa_family) },
        None => return None,
    };
    let size = match family {
        libc::AF_INET => size_of::<libc::sockaddr_in>(),
        libc::AF_INET6 => size_of::<libc::sockaddr_in6>(),
        _ => return None,
    };
    if len > size || (mask_family.is_none() && len != size) {
        return None;
    }
    let mut storage = SockAddrStorage::zeroed();
    // SAFETY: len <= size, and size matches the chosen IP address family.
    // Zero-extension restores any missing trailing netmask bytes.
    unsafe {
        let sa = storage.view_as::<libc::sockaddr_storage>();
        std::ptr::copy_nonoverlapping(raw.cast::<u8>(), std::ptr::from_mut(sa).cast(), len);
        sa.ss_family = family as _;
        sa.ss_len = size as _;
        SockAddr::new(storage, size as _).as_socket()
    }
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv4Addr, ptr};

    use super::*;

    #[test]
    fn immediate_cancel_releases_callbacks() {
        // No requirement to have seen the first path before dropping a monitor.
        let (live, released) = mpsc::channel::<()>();
        for _ in 0..32 {
            let capture = live.clone();
            let monitor = PathMonitor::start(move |_| {
                let _keep_alive = &capture;
            })
            .unwrap();
            drop(monitor);
        }
        drop(live);
        assert_eq!(
            released.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    /// Drop must not reclaim a block's captures underneath an in-flight update.
    #[test]
    fn cancel_during_native_callback() {
        let (started, started_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel::<()>();
        let (released, released_rx) = mpsc::channel::<()>();
        let monitor = PathMonitor::start(move |path| {
            let _keep_alive = &released;
            if started.send(path).is_ok() {
                let _ = resume_rx.recv();
            }
        })
        .unwrap();
        let deadline = Duration::from_secs(5);
        let path = started_rx
            .recv_timeout(deadline)
            .expect("initial OS callback");
        println!("initial path: {path:?}");
        drop(started_rx);
        drop(monitor);
        assert!(matches!(
            released_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(resume);
        assert_eq!(
            released_rx.recv_timeout(deadline),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn zero_extend_truncated_ipv4_netmask() {
        // The last octet and trailing padding are not part of this sockaddr.
        let len = std::mem::offset_of!(libc::sockaddr_in, sin_addr) + 3;
        let raw = libc::sockaddr_in {
            sin_len: len as u8,
            sin_family: 0,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes([255, 255, 255, 0x55]),
            },
            sin_zero: [0x55; 8],
        };
        // SAFETY: raw is aligned and covers at least sin_len bytes.
        let addr = unsafe { socket_addr(ptr::from_ref(&raw).cast(), Some(libc::AF_INET)) }.unwrap();
        assert_eq!(addr.ip(), Ipv4Addr::new(255, 255, 255, 0));
    }
}
