//! Temporary iOS backend for <https://github.com/n0-computer/net-tools/issues/261>.
//! Enumerate real interfaces using netdev's public iOS implementation, not
//! macOS routing constants. Unknown default routes/gateways remain None.

use std::net::IpAddr;

pub(super) use super::netdev_impl::get_state;
use super::{DefaultRouteDetails, HomeRouter};

pub(super) async fn default_route() -> Option<DefaultRouteDetails> {
    let interface = netdev::get_default_interface().ok()?;
    Some(DefaultRouteDetails {
        interface_name: interface.name,
    })
}

pub(super) fn home_router() -> Option<HomeRouter> {
    let gateway = netdev::get_default_gateway().ok()?;
    let gateway = gateway
        .ipv4
        .into_iter()
        .map(IpAddr::V4)
        .chain(gateway.ipv6.into_iter().map(IpAddr::V6))
        .next()?;
    Some(HomeRouter {
        gateway,
        my_ip: super::netdev_impl::local_ip(),
    })
}
