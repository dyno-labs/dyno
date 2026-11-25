//! This machine's address inside the device link's tunnel, where the phone's Safari can load a
//! page from this machine with no network path to it.
//!
//! A phone run has two connections. The device link, from this machine to the phone, records the
//! phone and opens Safari. The web path, from the phone to this machine, is how Safari gets the
//! page that dyno serves. On the local network the web path needs the phone to reach this machine
//! there, which a guest Wi-Fi or a cloud device lab often does not allow. But the device link is
//! a tunnel in which each end has an address, so it can be the web path too.
//!
//! There can be two tunnels to one phone, and only one of them can serve a page:
//!
//! - **dyno's own.** [`crate::phone`] opens the phone's CoreDevice tunnel itself, and runs it
//!   through `idevice`'s software tunnel. Its handshake gives this machine's address in it
//!   (`TunnelInfo::client_address`). But the tunnel's packets end in a TCP stack inside dyno
//!   (the `jktcp` crate, 0.1.8), not in the operating system, so no server can be bound at that
//!   address. And that stack only opens connections: a packet for a port that it did not open a
//!   connection from matches no state and is dropped (`Adapter::process_tcp_packet_from_payload`
//!   looks up `states` by the destination port), and its handle has `connect` and no `listen` or
//!   `accept`. So the phone cannot connect to this machine through dyno's own tunnel. To serve
//!   there, the stack would need a passive open: a port to listen on, a SYN-ACK in answer to a
//!   SYN for that port, a connection state made from the phone's SYN, and an `accept` that
//!   gives each new connection as a stream. dyno's servers would also have to read from those
//!   streams, not from the operating system's sockets.
//! - **A Mac's own.** macOS keeps a tunnel of its own to a phone that it is paired with, as a
//!   network interface. A server bound to the Mac's address there gets the phone's connections:
//!   Safari on a phone loaded a page from it in a test by hand, on a host that the phone could
//!   not reach on any network. Linux has no such tunnel.
//!
//! So only a Mac has a tunnel address to serve at, and only macOS knows it. dyno finds it in two
//! steps, with no interface name. `xcrun devicectl list devices` gives the phone's address in the
//! tunnel ([`crate::devicectl::tunnel_address`]). Then the operating system's own routing gives
//! this machine's address on the way to it ([`route_source`]). The two are a tunnel's two ends
//! only when [`is_tunnel_pair`] says so: two different IPv6 addresses that a URL can name, in one
//! /64 network. A Mac whose tunnel has gone can still have a default route for the phone's
//! address, out to the internet, and a phone address that is this machine's own is no phone's.
//!
//! The address is read once, before the page is served. The tunnel can go away after that, as it
//! does when nothing has used it for a while, so the caller serves at the local network address
//! as well, and asks the browser for it when no request comes to this one (`dyno-serve`).

use std::ffi::OsStr;
use std::net::{IpAddr, Ipv6Addr, UdpSocket};

use crate::devicectl;

/// Why this machine has no tunnel address to serve a page to the phone at.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoTunnelAddress {
    #[error(
        "this machine has no tunnel of its own to the phone, which only a Mac has, and dyno's own tunnel takes no connections from the phone"
    )]
    NoSystemTunnel,
    #[error("the phone gave no UDID to find its tunnel by")]
    NoUdid,
    #[error("devicectl gave no tunnel address for the phone: {0}")]
    Devicectl(String),
    #[error(
        "devicectl gave {0} as the phone's tunnel address, which is not an address of another machine in a tunnel"
    )]
    NotAPhoneAddress(IpAddr),
    #[error("this machine has no route to the phone's tunnel address {phone}: {detail}")]
    NoRoute { phone: IpAddr, detail: String },
    #[error(
        "the route to the phone's tunnel address {phone} starts at {host}, which is not this machine's end of that tunnel"
    )]
    NotInTheTunnel { phone: IpAddr, host: IpAddr },
}

/// This Mac's address inside its own tunnel to the phone with this UDID: the address the phone's
/// Safari reaches a server on this Mac at. `xcrun` is the program that runs devicectl.
pub(crate) fn host_address(xcrun: &OsStr, udid: Option<&str>) -> Result<IpAddr, NoTunnelAddress> {
    let udid = udid.ok_or(NoTunnelAddress::NoUdid)?;
    let phone = devicectl::tunnel_address(xcrun, udid).map_err(NoTunnelAddress::Devicectl)?;
    host_address_for(phone, route_source)
}

/// This machine's address in the tunnel whose other end has the address `phone`. `route` gives
/// the address of this machine that the operating system sends to an address from.
fn host_address_for(
    phone: IpAddr,
    route: impl Fn(IpAddr) -> std::io::Result<IpAddr>,
) -> Result<IpAddr, NoTunnelAddress> {
    if !is_tunnel_address(phone) {
        return Err(NoTunnelAddress::NotAPhoneAddress(phone));
    }
    let host = route(phone).map_err(|error| NoTunnelAddress::NoRoute {
        phone,
        detail: error.to_string(),
    })?;
    if !is_tunnel_pair(host, phone) {
        return Err(NoTunnelAddress::NotInTheTunnel { phone, host });
    }
    Ok(host)
}

/// The address of this machine that the operating system sends to `to` from. Connecting a UDP
/// socket sends nothing: it only makes the system choose a route, and so a local address.
fn route_source(to: IpAddr) -> std::io::Result<IpAddr> {
    let any: IpAddr = match to {
        IpAddr::V4(_) => std::net::Ipv4Addr::UNSPECIFIED.into(),
        IpAddr::V6(_) => Ipv6Addr::UNSPECIFIED.into(),
    };
    let socket = UdpSocket::bind((any, 0))?;
    // The port is the discard service's. Nothing is sent to it.
    socket.connect((to, 9))?;
    Ok(socket.local_addr()?.ip())
}

/// Whether an address can be one end of a tunnel: an IPv6 address of one machine that a URL can
/// name. So not the loopback address, which is this machine's own, not the unspecified address,
/// not an address of a group, not an IPv4 address written as IPv6, and not a link-local address,
/// which needs an interface's name beside it.
fn is_tunnel_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
                && address.to_ipv4_mapped().is_none()
        }
        IpAddr::V4(_) => false,
    }
}

/// Whether two addresses are a tunnel's two ends: each can be one ([`is_tunnel_address`]), they
/// are two addresses and not one, and they are in one network. A CoreDevice tunnel gives its ends
/// IPv6 addresses in one /64 network, so the first 64 bits are the same.
fn is_tunnel_pair(host: IpAddr, phone: IpAddr) -> bool {
    is_tunnel_address(host)
        && is_tunnel_address(phone)
        && host != phone
        && same_network(host, phone)
}

/// Whether two addresses have the same first 64 bits. An IPv4 address is in no tunnel's network.
fn same_network(one: IpAddr, other: IpAddr) -> bool {
    match (one, other) {
        (IpAddr::V6(one), IpAddr::V6(other)) => one.segments()[..4] == other.segments()[..4],
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_two_ends_of_a_tunnel_are_in_one_network() {
        assert!(same_network(
            address("fd7b:e5b:6f53::2"),
            address("fd7b:e5b:6f53::1")
        ));
        assert!(same_network(
            address("fd7b:e5b:6f53:0:1:2:3:4"),
            address("fd7b:e5b:6f53::1")
        ));
    }

    #[test]
    fn an_address_of_another_network_is_not_in_the_tunnel() {
        // The fourth group is part of the network.
        assert!(!same_network(
            address("fd7b:e5b:6f53:1::2"),
            address("fd7b:e5b:6f53::1")
        ));
        assert!(!same_network(
            address("2001:db8::2"),
            address("fd7b:e5b:6f53::1")
        ));
    }

    #[test]
    fn an_ipv4_address_is_in_no_tunnel() {
        assert!(!same_network(address("192.0.2.2"), address("192.0.2.1")));
        assert!(!same_network(address("192.0.2.2"), address("fd7b::1")));
    }

    /// A route lookup that always gives this address.
    fn route_from(host: &'static str) -> impl Fn(IpAddr) -> std::io::Result<IpAddr> {
        move |_| Ok(address(host))
    }

    #[test]
    fn the_other_end_of_the_phone_s_tunnel_is_this_machine_s_address_there() {
        assert_eq!(
            host_address_for(address("fd7b:e5b:6f53::1"), route_from("fd7b:e5b:6f53::2")),
            Ok(address("fd7b:e5b:6f53::2"))
        );
    }

    #[test]
    fn the_route_is_looked_up_for_the_phone_s_address() {
        let asked = std::cell::Cell::new(None);
        let route = |to| {
            asked.set(Some(to));
            Ok(address("fd7b:e5b:6f53::2"))
        };
        host_address_for(address("fd7b:e5b:6f53::1"), route).unwrap();
        assert_eq!(asked.get(), Some(address("fd7b:e5b:6f53::1")));
    }

    #[test]
    fn a_phone_address_that_no_other_machine_can_have_is_refused_before_any_route() {
        let unusable = [
            "::1",
            "::",
            "ff02::1",
            "fe80::1",
            "::ffff:192.0.2.1",
            "192.0.2.1",
            "127.0.0.1",
        ];
        for phone in unusable {
            let route = |_| panic!("no route is looked up for {phone}");
            assert_eq!(
                host_address_for(address(phone), route),
                Err(NoTunnelAddress::NotAPhoneAddress(address(phone))),
                "{phone}"
            );
        }
    }

    #[test]
    fn a_route_that_starts_at_the_phone_s_own_address_is_not_a_tunnel() {
        // This machine has the address itself, so nothing is at the other end.
        let phone = address("fd7b:e5b:6f53::1");
        assert_eq!(
            host_address_for(phone, route_from("fd7b:e5b:6f53::1")),
            Err(NoTunnelAddress::NotInTheTunnel { phone, host: phone })
        );
    }

    #[test]
    fn a_route_that_starts_in_another_network_or_at_no_usable_address_is_not_the_tunnel() {
        let phone = address("fd7b:e5b:6f53::1");
        for host in [
            "2001:db8::2",
            "fd7b:e5b:6f53:1::2",
            "::1",
            "fe80::2",
            "192.0.2.2",
        ] {
            assert_eq!(
                host_address_for(phone, route_from(host)),
                Err(NoTunnelAddress::NotInTheTunnel {
                    phone,
                    host: address(host)
                }),
                "{host}"
            );
        }
    }

    #[test]
    fn a_phone_with_no_route_to_it_gives_no_address_and_the_reason() {
        let phone = address("fd7b:e5b:6f53::1");
        let no_route = |_| Err(std::io::Error::other("network is unreachable"));
        assert_eq!(
            host_address_for(phone, no_route),
            Err(NoTunnelAddress::NoRoute {
                phone,
                detail: "network is unreachable".to_owned()
            })
        );
    }

    #[test]
    fn the_route_to_an_address_of_this_machine_starts_at_that_address() {
        assert_eq!(
            route_source(address("127.0.0.1")).unwrap(),
            address("127.0.0.1")
        );
    }

    #[test]
    fn a_documentation_address_has_no_tunnel_behind_it_on_any_machine() {
        // RFC 3849: no tunnel has it. A machine with no IPv6 route has no route to it, and one
        // with a route to the internet starts from its own address there.
        let phone = address("2001:db8::1");
        match host_address_for(phone, route_source) {
            Err(NoTunnelAddress::NoRoute { phone: to, .. }) => assert_eq!(to, phone),
            Err(NoTunnelAddress::NotInTheTunnel { phone: to, host }) => {
                assert_eq!(to, phone);
                assert!(!same_network(host, phone));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_phone_with_no_udid_has_no_tunnel_to_find() {
        // The program is never run.
        let xcrun = OsStr::new("no-such-xcrun-in-tests");
        assert_eq!(host_address(xcrun, None), Err(NoTunnelAddress::NoUdid));
    }

    #[test]
    fn a_devicectl_that_cannot_run_gives_no_tunnel_address() {
        let xcrun = OsStr::new("no-such-xcrun-in-tests");
        let error = host_address(xcrun, Some("00008120-000000000000001E")).unwrap_err();
        assert!(matches!(error, NoTunnelAddress::Devicectl(_)), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_tunnel_address_from_devicectl_counts_only_with_this_machine_s_end_of_it() {
        // The stub lists the phone at fd7b:e5b:6f53::1. This machine has no tunnel to it, so
        // the system's own routing gives no address in that network.
        let xcrun =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/stub-xcrun.sh");
        let phone = address("fd7b:e5b:6f53::1");
        match host_address(xcrun.as_os_str(), Some("00008120-000000000000001E")) {
            Err(NoTunnelAddress::NoRoute { phone: to, .. }) => assert_eq!(to, phone),
            Err(NoTunnelAddress::NotInTheTunnel { phone: to, .. }) => assert_eq!(to, phone),
            other => panic!("{other:?}"),
        }
    }
}
