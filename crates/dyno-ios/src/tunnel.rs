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
//! this machine's address on the way to it ([`route_source`]). That address is the tunnel's only
//! when it is in the phone's network, which [`same_network`] checks: a Mac whose tunnel has gone
//! can still have a default route for the phone's address, out to the internet.

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
    #[error("this machine has no route to the phone's tunnel address {phone}: {detail}")]
    NoRoute { phone: IpAddr, detail: String },
    #[error(
        "the route to the phone's tunnel address {phone} starts at {host}, which is not in the tunnel's network"
    )]
    NotInTheTunnel { phone: IpAddr, host: IpAddr },
}

/// This Mac's address inside its own tunnel to the phone with this UDID: the address the phone's
/// Safari reaches a server on this Mac at.
pub(crate) fn host_address(udid: Option<&str>) -> Result<IpAddr, NoTunnelAddress> {
    let udid = udid.ok_or(NoTunnelAddress::NoUdid)?;
    let phone = devicectl::tunnel_address(udid).map_err(NoTunnelAddress::Devicectl)?;
    host_address_for(phone)
}

/// This machine's address in the tunnel whose other end has the address `phone`.
fn host_address_for(phone: IpAddr) -> Result<IpAddr, NoTunnelAddress> {
    let host = route_source(phone).map_err(|error| NoTunnelAddress::NoRoute {
        phone,
        detail: error.to_string(),
    })?;
    if !same_network(host, phone) {
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

/// Whether two addresses are in one tunnel's network. A CoreDevice tunnel gives its two ends IPv6
/// addresses in one /64 network, so that is the test: the first 64 bits are the same. An IPv4
/// address is in no tunnel.
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

    /// Whether this machine has the IPv6 loopback address. A machine without one, as some
    /// containers are, cannot run the tests that route to it.
    fn has_ipv6_loopback() -> bool {
        UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).is_ok()
    }

    #[test]
    fn the_route_to_an_address_of_this_machine_starts_at_that_address() {
        assert_eq!(
            route_source(address("127.0.0.1")).unwrap(),
            address("127.0.0.1")
        );
        if has_ipv6_loopback() {
            assert_eq!(route_source(address("::1")).unwrap(), address("::1"));
        }
    }

    #[test]
    fn a_phone_in_this_machine_s_network_gives_the_address_on_the_way_to_it() {
        if !has_ipv6_loopback() {
            return;
        }
        // The loopback address stands in for a tunnel: both of its ends are this machine.
        assert_eq!(host_address_for(address("::1")), Ok(address("::1")));
    }

    #[test]
    fn a_phone_address_with_no_tunnel_behind_it_gives_no_address() {
        // A documentation address (RFC 3849): no tunnel has it. A machine with no IPv6 route has
        // no route to it, and one with a route to the internet starts from its own address there.
        let phone = address("2001:db8::1");
        match host_address_for(phone) {
            Err(NoTunnelAddress::NoRoute { phone: to, .. }) => assert_eq!(to, phone),
            Err(NoTunnelAddress::NotInTheTunnel { phone: to, host }) => {
                assert_eq!(to, phone);
                assert!(!same_network(host, phone));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_ipv4_phone_address_is_not_a_tunnel_s() {
        assert_eq!(
            host_address_for(address("127.0.0.1")),
            Err(NoTunnelAddress::NotInTheTunnel {
                phone: address("127.0.0.1"),
                host: address("127.0.0.1"),
            })
        );
    }

    #[test]
    fn a_phone_with_no_udid_has_no_tunnel_to_find() {
        assert_eq!(host_address(None), Err(NoTunnelAddress::NoUdid));
    }
}
