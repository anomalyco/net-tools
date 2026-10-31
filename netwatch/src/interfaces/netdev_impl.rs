//! Conversion from the `netdev` crate into our platform-agnostic types.
//!
//! This module also holds the interface enumeration and home-router lookup
//! shared by all `netdev`-capable platforms (linux, android, bsd, macos,
//! windows).
//!
//! This is the only module that depends on `netdev`. Everything it produces is
//! expressed in terms of the types defined in [`crate::interfaces`].

use std::net::IpAddr;

use super::{IFF_LOOPBACK, IFF_UP, Interface, IpNet, Ipv6AddrFlags, State, snapshot};
use crate::ip::LocalAddresses;

/// Converts netdev's IPv6 address flags into our mirrored [`Ipv6AddrFlags`].
///
/// This is a free function rather than a `From` impl on purpose: a public
/// `From<netdev::...>` would re-expose the `netdev` type in our public API,
/// which is exactly what the [`Ipv6AddrFlags`] mirror exists to avoid.
fn to_ipv6_addr_flags(flags: netdev::interface::ipv6_addr_flags::Ipv6AddrFlags) -> Ipv6AddrFlags {
    Ipv6AddrFlags {
        deprecated: flags.deprecated,
        temporary: flags.temporary,
        tentative: flags.tentative,
        duplicated: flags.duplicated,
        permanent: flags.permanent,
    }
}

/// Converts a [`netdev::Interface`] into our platform-agnostic [`Interface`].
///
/// Addresses are sorted (IPv4 first, then IPv6, each by address) so that
/// comparisons between successive snapshots are stable.
fn to_interface(iface: netdev::Interface) -> Interface {
    // netdev keeps these three IPv6 arrays parallel, one entry per address.
    // The zip below relies on that; assert it so a netdev change that breaks
    // the invariant surfaces in tests rather than silently dropping addresses.
    debug_assert_eq!(iface.ipv6.len(), iface.ipv6_scope_ids.len());
    debug_assert_eq!(iface.ipv6.len(), iface.ipv6_addr_flags.len());

    let mut v4: Vec<IpNet> = iface.ipv4.iter().copied().map(IpNet::V4).collect();
    let mut v6: Vec<IpNet> = iface
        .ipv6
        .iter()
        .copied()
        .zip(iface.ipv6_scope_ids.iter().copied())
        .zip(iface.ipv6_addr_flags.iter().copied())
        .map(|((net, scope_id), flags)| IpNet::V6 {
            net,
            scope_id,
            flags: to_ipv6_addr_flags(flags),
        })
        .collect();

    // Sort each family by address so successive snapshots compare equal, then
    // concatenate as IPv4-first.
    v4.sort_by_key(IpNet::addr);
    v6.sort_by_key(IpNet::addr);
    let mut addrs = v4;
    addrs.append(&mut v6);

    Interface {
        name: iface.name,
        index: iface.index,
        flags: iface.flags,
        mac_addr: iface.mac_addr.as_ref().map(|a| a.octets()),
        addrs,
    }
}

/// Enumerates the machine's network interfaces and assembles the [`State`].
pub(super) async fn get_state() -> State {
    let interfaces = netdev::interface::get_interfaces()
        .into_iter()
        .map(to_interface)
        .map(|i| (i.name.clone(), i))
        .collect();
    let route = super::default_route_interface().await;
    snapshot::state(interfaces, route)
}

/// The shared home-router lookup for linux, android and windows.
///
/// BSD platforms do not use this as `netdev` cannot yet determine their default
/// gateway, so they provide their own implementation.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "windows"))]
pub(super) fn home_router() -> Option<super::HomeRouter> {
    let gateway = netdev::get_default_gateway().ok()?;
    let gateway = gateway
        .ipv4
        .iter()
        .copied()
        .map(IpAddr::V4)
        .chain(gateway.ipv6.iter().copied().map(IpAddr::V6))
        .next()?;

    Some(super::HomeRouter {
        gateway,
        my_ip: local_ip(),
    })
}

/// The local IP address of this machine, as reported by `netdev`.
pub(super) fn local_ip() -> Option<IpAddr> {
    netdev::net::ip::get_local_ipaddr()
}

impl LocalAddresses {
    /// Returns the machine's IP addresses.
    ///
    /// If there are no regular addresses it will return any IPv4 link-local or
    /// IPv6 unique-local addresses, because we know of environments where these
    /// are used with NAT to provide connectivity.
    pub fn new() -> Self {
        let ifaces = netdev::interface::get_interfaces();
        snapshot::local_addresses(
            ifaces
                .iter()
                .filter(|iface| iface.flags & IFF_UP != 0)
                .flat_map(|iface| {
                    let loopback = iface.flags & IFF_LOOPBACK != 0;
                    iface
                        .ipv4
                        .iter()
                        .map(|a| IpAddr::V4(a.addr()))
                        .chain(iface.ipv6.iter().map(|a| IpAddr::V6(a.addr())))
                        .map(move |ip| (ip, loopback))
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_addresses() {
        let addrs = LocalAddresses::new();
        dbg!(&addrs);
        assert!(!addrs.loopback.is_empty());
        assert!(!addrs.regular.is_empty());
    }
}
