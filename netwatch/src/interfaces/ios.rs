//! iOS exposes addresses through `getifaddrs` and route observations through
//! Network.framework. Neither requires a routing socket or a private ioctl.
use std::{
    collections::HashMap,
    ffi::CStr,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    ptr,
};

use super::{DefaultRouteDetails, HomeRouter, Interface, IpNet, Ipv4Net, Ipv6Net, State, snapshot};
use crate::{
    ios::{Path, socket_addr},
    ip::LocalAddresses,
};

struct IfAddrs(*mut libc::ifaddrs);

impl Drop for IfAddrs {
    fn drop(&mut self) {
        // SAFETY: this guard owns the list from a successful getifaddrs call.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

fn interfaces() -> HashMap<String, Interface> {
    let mut head = ptr::null_mut();
    // SAFETY: getifaddrs initializes head on success; the guard releases it.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "getifaddrs failed");
        return HashMap::new();
    }
    let records = IfAddrs(head);
    let mut ifaces = HashMap::new();
    let mut next = records.0;
    // SAFETY: each node and its pointed-to fields belong to the live list.
    while let Some(record) = unsafe { next.as_ref() } {
        next = record.ifa_next;
        // SAFETY: ifa_name is a NUL-terminated interface name.
        let name = unsafe { CStr::from_ptr(record.ifa_name) }.to_string_lossy();
        let iface = ifaces
            .entry(name.into_owned())
            .or_insert_with_key(|name| Interface {
                name: name.clone(),
                // SAFETY: same live NUL-terminated name.
                index: unsafe { libc::if_nametoindex(record.ifa_name) },
                flags: record.ifa_flags,
                mac_addr: None,
                addrs: Vec::new(),
            });
        // SAFETY: the address and optional (possibly truncated) netmask come
        // from the same live ifaddrs record.
        let ip = unsafe { socket_addr(record.ifa_addr, None) };
        let net = match ip {
            Some(SocketAddr::V4(a)) => {
                let mask = unsafe { socket_addr(record.ifa_netmask, Some(libc::AF_INET)) };
                let mask = match mask {
                    Some(SocketAddr::V4(m)) => *m.ip(),
                    _ => Ipv4Addr::UNSPECIFIED,
                };
                Ipv4Net::with_netmask(*a.ip(), mask).ok().map(IpNet::V4)
            }
            Some(SocketAddr::V6(a)) => {
                let mask = unsafe { socket_addr(record.ifa_netmask, Some(libc::AF_INET6)) };
                let mask = match mask {
                    Some(SocketAddr::V6(m)) => *m.ip(),
                    _ => Ipv6Addr::UNSPECIFIED,
                };
                Ipv6Net::with_netmask(*a.ip(), mask)
                    .ok()
                    .map(|net| IpNet::V6 {
                        net,
                        scope_id: if a.scope_id() == 0 && a.ip().is_unicast_link_local() {
                            iface.index
                        } else {
                            a.scope_id()
                        },
                        // No public per-address IPv6 flags ioctl on iOS.
                        flags: Default::default(),
                    })
            }
            None => {
                // SAFETY: borrowed from the list; length checked before use.
                if let Some(mac) = unsafe { mac_addr(record.ifa_addr) } {
                    iface.mac_addr = Some(mac);
                }
                None
            }
        };
        if let Some(net) = net {
            iface.addrs.push(net);
        }
    }
    for iface in ifaces.values_mut() {
        iface.addrs.sort_by_key(IpNet::addr);
    }
    ifaces
}

/// # Safety
/// `raw` is a nullable borrowed sockaddr from getifaddrs.
unsafe fn mac_addr(raw: *const libc::sockaddr) -> Option<[u8; 6]> {
    if raw.is_null() {
        return None;
    }
    let offset = std::mem::offset_of!(libc::sockaddr_dl, sdl_data);
    // SAFETY: `sa_len` bounds both the fixed header and the variable-length
    // link-layer address that follows the interface name.
    unsafe {
        let len = usize::from((*raw).sa_len);
        if len < offset || i32::from((*raw).sa_family) != libc::AF_LINK {
            return None;
        }
        let dl = raw.cast::<libc::sockaddr_dl>();
        let start = offset + usize::from((*dl).sdl_nlen);
        if (*dl).sdl_alen != 6 || start + 6 > len {
            return None;
        }
        Some(raw.cast::<u8>().add(start).cast::<[u8; 6]>().read())
    }
}

pub(crate) fn state_for(path: Option<Path>) -> State {
    let ifaces = interfaces();
    let route = path.filter(|p| p.interface_index != 0).and_then(|p| {
        ifaces
            .values()
            .find(|i| i.index == p.interface_index)
            .map(|i| i.name.clone())
    });
    snapshot::state(ifaces, route)
}

pub(super) async fn get_state() -> State {
    state_for(Path::current().await)
}

pub(super) async fn default_route() -> Option<DefaultRouteDetails> {
    let index = Path::current().await?.interface_index;
    let mut buf = [0; libc::IF_NAMESIZE];
    // SAFETY: buffer holds IF_NAMESIZE bytes; if_indextoname NUL-terminates on success.
    let name = unsafe { libc::if_indextoname(index, buf.as_mut_ptr()) };
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_str().ok()?.to_owned();
    Some(DefaultRouteDetails {
        interface_name: name,
    })
}

pub(super) fn home_router() -> Option<HomeRouter> {
    router_for(Path::current_blocking()?)
}

fn router_for(path: Path) -> Option<HomeRouter> {
    let mut gw = path.gateway?;
    // Ask the kernel which source address it would use for this gateway, not
    // for an unrelated Internet host. UDP connect does not transmit a packet.
    let any_ip: IpAddr = if gw.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    };
    gw.set_port(9);
    let my_ip = UdpSocket::bind((any_ip, 0))
        .and_then(|socket| {
            socket.connect(gw)?;
            socket.local_addr()
        })
        .ok()
        .map(|addr| addr.ip());
    Some(HomeRouter {
        gateway: gw.ip(),
        my_ip,
    })
}

impl LocalAddresses {
    /// Returns the machine's IP addresses, including IPv4 link-local or IPv6
    /// unique-local addresses as a fallback when there are no regular addresses.
    pub fn new() -> Self {
        snapshot::interface_ips(&interfaces())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_path_keeps_interface_addresses() {
        let state = state_for(None);
        assert!(state.default_route_interface.is_none());
        assert!(
            state
                .local_addresses
                .loopback
                .iter()
                .any(|ip| ip.is_loopback())
        );
        assert!(router_for(Path::default()).is_none());
    }

    #[test]
    fn reported_gateway_uses_its_own_address_family() {
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ] {
            let router = router_for(Path {
                gateway: Some(SocketAddr::new(ip, 0)),
                ..Path::default()
            })
            .unwrap();
            assert_eq!(router.gateway, ip);
            assert_eq!(router.my_ip, Some(ip));
        }
    }
}
