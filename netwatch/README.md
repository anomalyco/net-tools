# Netwatch

`netwatch` is a cross-platform library for monitoring of networking interfaces
and route changes.

Used in [iroh](https://github.com/n0-computer/iroh), created with love by the
[n0 team](https://n0.computer/).

## iOS

On iOS 12 and later, `Monitor` receives network changes from Network.framework.
The preferred interface in use by the satisfied default path is a best-effort
answer for the default route, not a route-table dump. Interface addresses come
from `getifaddrs` even without a satisfied path. Per-address IPv6 state flags
aren't available through that API. On older systems `Monitor::new()` returns
an error; address enumeration still works.

`HomeRouter::new()` uses gateways reported by the path (iOS 13 and later).
This synchronous query waits at most one second for an initial path and returns
`None` if no gateway is reported, as can happen on cellular or a simulator.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
