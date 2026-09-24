//! WovenHat Stage 9 IPv4 network stack.
//!
//! Stage 9 keeps the shell-first recovery path while exposing smoltcp sockets
//! to Ring-3 processes through a small kernel ABI.  The implementation uses
//! owned buffers so sockets can be created and destroyed dynamically without
//! static-lifetime bookkeeping in user processes.

use crate::irq_lock::IrqMutex as Mutex;
use alloc::{vec, vec::Vec};
use smoltcp::{
    iface::{Config, Interface, SocketHandle, SocketSet},
    phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::{dhcpv4, dns, icmp, tcp, udp},
    time::Instant,
    wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address},
};
use spin::Once;

use crate::{
    timer,
    virtio_net::{self, MAX_FRAME},
};

pub const DEFAULT_IPV4: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub const DEFAULT_GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
pub const DEFAULT_DNS: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);
pub const DEFAULT_PREFIX: u8 = 24;
pub const MAX_USER_SOCKETS: usize = 16;
pub const SOCKET_BUFFER_BYTES: usize = 4096;
pub const UDP_META_SLOTS: usize = 8;
// Closed descriptors retain their TCP transport for a bounded graceful drain.
// This is a resource-retirement bound, not an acceptance-test timeout.
const TCP_CLOSE_GRACE_TICKS: u64 = timer::FREQUENCY_HZ as u64 * 30;

pub fn default_cidr() -> IpCidr {
    IpCidr::new(IpAddress::Ipv4(DEFAULT_IPV4), DEFAULT_PREFIX)
}

pub struct VirtioSmolDevice {
    rx: [u8; MAX_FRAME],
}
impl VirtioSmolDevice {
    pub const fn new() -> Self {
        Self { rx: [0; MAX_FRAME] }
    }
}

pub struct WovenRxToken<'a> {
    data: &'a mut [u8],
}
pub struct WovenTxToken;

impl RxToken for WovenRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.data)
    }
}
impl TxToken for WovenTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut frame = [0u8; MAX_FRAME];
        let usable = core::cmp::min(len, MAX_FRAME);
        let result = f(&mut frame[..usable]);
        let _ = virtio_net::transmit(&frame[..usable]);
        result
    }
}

impl Device for VirtioSmolDevice {
    type RxToken<'a>
        = WovenRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = WovenTxToken
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let len = virtio_net::receive_into(&mut self.rx)?;
        Some((
            WovenRxToken {
                data: &mut self.rx[..len],
            },
            WovenTxToken,
        ))
    }
    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        // Returning Some(token) is a contract that the token can actually
        // submit a frame. The legacy virtio-net transport currently owns one
        // polling-mode TX descriptor, so defer smoltcp transmission while that
        // descriptor is still outstanding instead of handing out a token that
        // would silently drop the frame in `consume()`.
        virtio_net::tx_available().then_some(WovenTxToken)
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        // VirtioSmolDevice currently submits complete Ethernet frames and does
        // not request or implement virtio-net checksum offload.  Therefore
        // smoltcp must compute/verify IPv4, ICMP, UDP and TCP checksums in
        // software.  `ChecksumCapabilities::ignored()` disables that work and
        // produces invalid live-network traffic (most visibly DHCP timeouts).
        //
        // For Ethernet devices smoltcp expects this value to include the
        // 14-byte Ethernet header, so 1514 corresponds to the normal 1500-byte
        // IPv4 MTU.
        caps.max_transmission_unit = 1514;
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}

// The candidate Wi-Fi transport has the same feature boundary as its backend.
#[cfg(not(feature = "stage13-9-test"))]
type NetTransport = VirtioSmolDevice;
#[cfg(feature = "stage13-9-test")]
pub use wifi_transport::{NetTransport, WifiNetDevice};

#[cfg(feature = "stage13-9-test")]
mod wifi_transport {
    use super::*;
    use crate::{wifi_session::WifiSession, wifi_smol};

    pub struct WifiNetDevice {
        session: WifiSession,
        epoch: u32,
        rx: [u8; wifi_smol::ETHERNET_MTU],
    }
    impl WifiNetDevice {
        pub fn new(session: WifiSession, epoch: u32) -> Self {
            Self {
                session,
                epoch,
                rx: [0; wifi_smol::ETHERNET_MTU],
            }
        }
        #[cfg(feature = "stage13-9-test")]
        pub fn session_mut(&mut self) -> &mut WifiSession {
            &mut self.session
        }
    }

    #[allow(clippy::large_enum_variant)]
    pub enum NetTransport {
        Virtio(VirtioSmolDevice),
        Wifi(WifiNetDevice),
    }
    pub enum NetRxToken<'a> {
        Virtio(WovenRxToken<'a>),
        Wifi(wifi_smol::WifiRxToken<'a>),
    }
    pub enum NetTxToken<'a> {
        Virtio(WovenTxToken),
        Wifi(wifi_smol::WifiTxToken<'a>),
    }
    impl RxToken for NetRxToken<'_> {
        fn consume<R, F>(self, f: F) -> R
        where
            F: FnOnce(&[u8]) -> R,
        {
            match self {
                Self::Virtio(t) => t.consume(f),
                Self::Wifi(t) => t.consume(f),
            }
        }
    }
    impl TxToken for NetTxToken<'_> {
        fn consume<R, F>(self, len: usize, f: F) -> R
        where
            F: FnOnce(&mut [u8]) -> R,
        {
            match self {
                Self::Virtio(t) => t.consume(len, f),
                Self::Wifi(t) => t.consume(len, f),
            }
        }
    }
    impl Device for NetTransport {
        type RxToken<'a>
            = NetRxToken<'a>
        where
            Self: 'a;
        type TxToken<'a>
            = NetTxToken<'a>
        where
            Self: 'a;

        fn receive(
            &mut self,
            timestamp: Instant,
        ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
            match self {
                Self::Virtio(d) => d
                    .receive(timestamp)
                    .map(|(r, t)| (NetRxToken::Virtio(r), NetTxToken::Virtio(t))),
                Self::Wifi(d) => {
                    let len = match d.session.receive_ethernet(d.epoch, &mut d.rx) {
                        Ok(Some(n)) => n,
                        Ok(None) | Err(_) => return None,
                    };
                    Some((
                        NetRxToken::Wifi(wifi_smol::WifiRxToken::new(&mut d.rx[..len])),
                        NetTxToken::Wifi(wifi_smol::WifiTxToken::new(&mut d.session, d.epoch)),
                    ))
                }
            }
        }
        fn transmit(&mut self, timestamp: Instant) -> Option<Self::TxToken<'_>> {
            match self {
                Self::Virtio(d) => d.transmit(timestamp).map(NetTxToken::Virtio),
                Self::Wifi(d) => d
                    .session
                    .is_active_epoch(d.epoch)
                    .then_some(NetTxToken::Wifi(wifi_smol::WifiTxToken::new(
                        &mut d.session,
                        d.epoch,
                    ))),
            }
        }
        fn capabilities(&self) -> DeviceCapabilities {
            match self {
                Self::Virtio(d) => d.capabilities(),
                Self::Wifi(_) => {
                    let mut c = DeviceCapabilities::default();
                    c.medium = Medium::Ethernet;
                    c.max_transmission_unit = wifi_smol::ETHERNET_MTU;
                    c.checksum = ChecksumCapabilities::default();
                    c
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketKind {
    Udp = 1,
    Tcp = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SocketError {
    Offline,
    Invalid,
    NoSlot,
    WrongOwner,
    WrongKind,
    NotBound,
    NotConnected,
    WouldBlock,
    BufferFull,
    Address,
}

#[derive(Clone, Copy)]
struct UserSocket {
    owner: u64,
    handle: SocketHandle,
    kind: SocketKind,
    peer: Option<IpEndpoint>,
    generation: u32,
    async_refs: u16,
    closing: bool,
    close_started: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketToken {
    slot: u16,
    generation: u32,
    owner: u64,
}

struct Runtime {
    iface: Interface,
    device: NetTransport,
    sockets: SocketSet<'static>,
    user: [Option<UserSocket>; MAX_USER_SOCKETS],
    echo_handle: Option<SocketHandle>,
    echo_port: u16,
    echo_packets: u64,
    dhcp_handle: Option<SocketHandle>,
    dns_handle: Option<SocketHandle>,
    dns_queries: [Option<dns::QueryHandle>; 4],
    dhcp_enabled: bool,
    using_dhcp: bool,
    ipv4: Ipv4Address,
    prefix: u8,
    gateway: Ipv4Address,
    dns_server: Ipv4Address,
    next_ephemeral: u16,
    next_socket_generation: u32,
    ping_handle: Option<SocketHandle>,
    ping_pending: Option<(Ipv4Address, u16, u64)>,
    ping_sequence: u16,
}
static RUNTIME: Once<Mutex<Runtime>> = Once::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EchoError {
    NetworkOffline,
    AlreadyConfigured,
    BindFailed,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NetStats {
    pub online: bool,
    pub echo_active: bool,
    pub echo_port: u16,
    pub echo_packets: u64,
    pub user_sockets: usize,
    pub dhcp_enabled: bool,
    pub using_dhcp: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetInfo {
    pub ipv4: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub prefix: u8,
    pub dhcp_enabled: u8,
    pub using_dhcp: u8,
    pub online: u8,
    pub mac: [u8; 6],
    pub _reserved: [u8; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    Transport(virtio_net::InitError),
    Route,
}

pub fn init() -> Result<(), InitError> {
    if RUNTIME.get().is_some() {
        return Ok(());
    }
    virtio_net::init().map_err(InitError::Transport)?;

    let mac = EthernetAddress(virtio_net::mac_address());
    #[cfg(feature = "stage13-9-test")]
    let mut device = NetTransport::Virtio(VirtioSmolDevice::new());
    #[cfg(not(feature = "stage13-9-test"))]
    let mut device = VirtioSmolDevice::new();
    let mut config = Config::new(mac.into());
    // Previously a fixed constant (0x5748_4f53_4e45_5435), which made TCP
    // initial sequence numbers and smoltcp's internal randomized choices
    // predictable to a network attacker on every boot. See `entropy.rs`.
    config.random_seed = crate::entropy::random_u64();
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(default_cidr());
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(DEFAULT_GATEWAY)
        .map_err(|_| InitError::Route)?;

    let mut sockets = SocketSet::new(Vec::new());

    // Stage 6 DHCP client. We keep the known-good QEMU static address until a
    // lease is actually acquired, so losing DHCP cannot take down recovery.
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());

    // QEMU user networking provides DNS proxy 10.0.2.3. DNS queries are
    // asynchronous and may be started/polled by the userspace ABI.
    let dns_servers = [IpAddress::Ipv4(DEFAULT_DNS)];
    let dns_queries = (0..4).map(|_| None).collect::<Vec<_>>();
    let dns_handle = sockets.add(dns::Socket::new(&dns_servers, dns_queries));

    let ping_rx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 4], vec![0; 1024]);
    let ping_tx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 4], vec![0; 1024]);
    let mut ping_socket = icmp::Socket::new(ping_rx, ping_tx);
    ping_socket
        .bind(icmp::Endpoint::Ident(0x5748))
        .map_err(|_| InitError::Route)?;
    let ping_handle = sockets.add(ping_socket);

    RUNTIME.call_once(|| {
        Mutex::with_rank(
            Runtime {
                iface,
                device,
                sockets,
                user: [None; MAX_USER_SOCKETS],
                echo_handle: None,
                echo_port: 0,
                echo_packets: 0,
                dhcp_handle: Some(dhcp_handle),
                dns_handle: Some(dns_handle),
                dns_queries: [None; 4],
                dhcp_enabled: true,
                using_dhcp: false,
                ipv4: DEFAULT_IPV4,
                prefix: DEFAULT_PREFIX,
                gateway: DEFAULT_GATEWAY,
                dns_server: DEFAULT_DNS,
                next_ephemeral: 49152,
                next_socket_generation: 1,
                ping_handle: Some(ping_handle),
                ping_pending: None,
                ping_sequence: 0,
            },
            20,
        )
    });
    Ok(())
}

pub fn poll() {
    virtio_net::poll();
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let mut runtime = runtime.lock();

    {
        let Runtime {
            iface,
            device,
            sockets,
            ..
        } = &mut *runtime;
        let _ = iface.poll(now(), device, sockets);
    }

    // Apply DHCP only after a valid lease arrives. Deconfiguration falls back
    // to the static QEMU topology so the diagnostic shell stays reachable.
    if runtime.dhcp_enabled {
        if let Some(handle) = runtime.dhcp_handle {
            // Copy lease data out of the DHCP socket before mutating the rest
            // of Runtime. This keeps the SocketSet mutable borrow tightly scoped.
            let update = {
                let socket = runtime.sockets.get_mut::<dhcpv4::Socket>(handle);
                match socket.poll() {
                    Some(dhcpv4::Event::Configured(config)) => {
                        let dns = config.dns_servers.first().copied().unwrap_or(DEFAULT_DNS);
                        Some(Some((
                            config.address,
                            config.router.unwrap_or(DEFAULT_GATEWAY),
                            dns,
                        )))
                    }
                    Some(dhcpv4::Event::Deconfigured) => Some(None),
                    None => None,
                }
            };

            match update {
                Some(Some((address, router, dns))) => {
                    runtime.iface.update_ip_addrs(|addrs| {
                        addrs.clear();
                        let _ = addrs.push(IpCidr::Ipv4(address));
                    });
                    runtime.iface.routes_mut().remove_default_ipv4_route();
                    let _ = runtime.iface.routes_mut().add_default_ipv4_route(router);
                    runtime.ipv4 = address.address();
                    runtime.prefix = address.prefix_len();
                    runtime.gateway = router;
                    runtime.dns_server = dns;
                    runtime.using_dhcp = true;
                    if let Some(dns_handle) = runtime.dns_handle {
                        runtime
                            .sockets
                            .get_mut::<dns::Socket>(dns_handle)
                            .update_servers(&[IpAddress::Ipv4(dns)]);
                    }
                }
                Some(None) if runtime.using_dhcp => {
                    apply_static_locked(&mut runtime);
                }
                Some(None) | None => {}
            }
        }
    }

    if let Some(handle) = runtime.echo_handle {
        let mut reply = [0u8; 512];
        let received = {
            let socket = runtime.sockets.get_mut::<udp::Socket>(handle);
            socket.recv_slice(&mut reply).ok()
        };
        if let Some((len, remote)) = received {
            if runtime
                .sockets
                .get_mut::<udp::Socket>(handle)
                .send_slice(&reply[..len], remote)
                .is_ok()
            {
                runtime.echo_packets = runtime.echo_packets.saturating_add(1);
            }
        }
    }

    {
        let Runtime {
            iface,
            device,
            sockets,
            ..
        } = &mut *runtime;
        let _ = iface.poll(now(), device, sockets);
    }
    for index in 0..MAX_USER_SOCKETS {
        retire_closed_socket(&mut runtime, index);
    }
    drop(runtime);
    crate::async_network::network_progress();
}

/// Caller holds the runtime lock. Descriptor revocation is immediate, but
/// queued TCP bytes and FIN need a live transport until close completes.
/// Existing async references retain the socket even during owner teardown.
fn retire_closed_socket(runtime: &mut Runtime, index: usize) {
    let Some(entry) = runtime.user[index] else {
        return;
    };
    if !entry.closing || entry.async_refs != 0 {
        return;
    }
    if entry.kind == SocketKind::Tcp {
        let socket = runtime.sockets.get_mut::<tcp::Socket>(entry.handle);
        if entry.close_started.is_none() {
            #[cfg(feature = "stage10-7-test")]
            crate::serial::write_line(format_args!(
                "[S10.7-DIAG] graceful TCP close tx_queue={}",
                socket.send_queue()
            ));
            socket.close();
            runtime.user[index].as_mut().unwrap().close_started = Some(timer::ticks());
        }
        if socket.is_open() {
            let started = runtime.user[index].unwrap().close_started.unwrap();
            if timer::ticks().wrapping_sub(started) < TCP_CLOSE_GRACE_TICKS {
                return;
            }
            // A dead peer cannot retain one of the bounded slots forever.
            // No async token remains; expiration deliberately abandons drain.
            socket.abort();
        }
    }
    let _ = runtime.sockets.remove(entry.handle);
    runtime.user[index] = None;
}

fn apply_static_locked(runtime: &mut Runtime) {
    runtime.iface.update_ip_addrs(|addrs| {
        addrs.clear();
        let _ = addrs.push(default_cidr());
    });
    runtime.iface.routes_mut().remove_default_ipv4_route();
    let _ = runtime
        .iface
        .routes_mut()
        .add_default_ipv4_route(DEFAULT_GATEWAY);
    runtime.ipv4 = DEFAULT_IPV4;
    runtime.prefix = DEFAULT_PREFIX;
    runtime.gateway = DEFAULT_GATEWAY;
    runtime.dns_server = DEFAULT_DNS;
    runtime.using_dhcp = false;
    if let Some(handle) = runtime.dns_handle {
        runtime
            .sockets
            .get_mut::<dns::Socket>(handle)
            .update_servers(&[IpAddress::Ipv4(DEFAULT_DNS)]);
    }
}

pub fn set_dhcp(enabled: bool) -> Result<(), SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    runtime.dhcp_enabled = enabled;
    if !enabled {
        if let Some(handle) = runtime.dhcp_handle {
            runtime.sockets.get_mut::<dhcpv4::Socket>(handle).reset();
        }
        apply_static_locked(&mut runtime);
    }
    Ok(())
}

pub fn start_udp_echo(port: u16) -> Result<(), EchoError> {
    if port == 0 {
        return Err(EchoError::BindFailed);
    }
    let Some(runtime) = RUNTIME.get() else {
        return Err(EchoError::NetworkOffline);
    };
    let mut runtime = runtime.lock();
    if runtime.echo_handle.is_some() {
        return if runtime.echo_port == port {
            Ok(())
        } else {
            Err(EchoError::AlreadyConfigured)
        };
    }
    let rx = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; UDP_META_SLOTS],
        vec![0; SOCKET_BUFFER_BYTES],
    );
    let tx = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; UDP_META_SLOTS],
        vec![0; SOCKET_BUFFER_BYTES],
    );
    let mut socket = udp::Socket::new(rx, tx);
    socket.bind(port).map_err(|_| EchoError::BindFailed)?;
    let handle = runtime.sockets.add(socket);
    runtime.echo_handle = Some(handle);
    runtime.echo_port = port;
    Ok(())
}

fn find_slot(runtime: &Runtime, owner: u64, id: u64) -> Result<UserSocket, SocketError> {
    let index = usize::try_from(id).map_err(|_| SocketError::Invalid)?;
    let socket = runtime
        .user
        .get(index)
        .and_then(|slot| *slot)
        .ok_or(SocketError::Invalid)?;
    if socket.owner != owner {
        return Err(SocketError::WrongOwner);
    }
    if socket.closing {
        return Err(SocketError::Invalid);
    }
    Ok(socket)
}

fn next_ephemeral(runtime: &mut Runtime) -> u16 {
    let port = runtime.next_ephemeral;
    runtime.next_ephemeral = if port >= 65534 { 49152 } else { port + 1 };
    port
}

pub fn socket_open(owner: u64, kind: SocketKind) -> Result<u64, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let slot = runtime
        .user
        .iter()
        .position(Option::is_none)
        .ok_or(SocketError::NoSlot)?;
    let handle = match kind {
        SocketKind::Udp => {
            let rx = udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; UDP_META_SLOTS],
                vec![0; SOCKET_BUFFER_BYTES],
            );
            let tx = udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; UDP_META_SLOTS],
                vec![0; SOCKET_BUFFER_BYTES],
            );
            runtime.sockets.add(udp::Socket::new(rx, tx))
        }
        SocketKind::Tcp => {
            let rx = tcp::SocketBuffer::new(vec![0; SOCKET_BUFFER_BYTES]);
            let tx = tcp::SocketBuffer::new(vec![0; SOCKET_BUFFER_BYTES]);
            runtime.sockets.add(tcp::Socket::new(rx, tx))
        }
    };
    let generation = runtime.next_socket_generation.max(1);
    runtime.next_socket_generation = generation.wrapping_add(1).max(1);
    runtime.user[slot] = Some(UserSocket {
        owner,
        handle,
        kind,
        peer: None,
        generation,
        async_refs: 0,
        closing: false,
        close_started: None,
    });
    Ok(slot as u64)
}

pub fn socket_bind(owner: u64, id: u64, port: u16) -> Result<(), SocketError> {
    if port == 0 {
        return Err(SocketError::Address);
    }
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_slot(&runtime, owner, id)?;
    match entry.kind {
        SocketKind::Udp => runtime
            .sockets
            .get_mut::<udp::Socket>(entry.handle)
            .bind(port)
            .map_err(|_| SocketError::Address),
        SocketKind::Tcp => runtime
            .sockets
            .get_mut::<tcp::Socket>(entry.handle)
            .listen(port)
            .map_err(|_| SocketError::Address),
    }
}

pub fn socket_connect(owner: u64, id: u64, endpoint: IpEndpoint) -> Result<(), SocketError> {
    if endpoint.port == 0 {
        return Err(SocketError::Address);
    }
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_slot(&runtime, owner, id)?;
    match entry.kind {
        SocketKind::Udp => {
            // A connected UDP socket needs a local endpoint before smoltcp can
            // transmit. Match normal socket semantics by assigning an
            // ephemeral port when userspace connects an unbound datagram
            // socket. Without this, send_slice reports an addressing failure;
            // treating that permanent condition as WouldBlock would make an
            // asynchronous sender retry forever.
            let needs_bind = !runtime.sockets.get::<udp::Socket>(entry.handle).is_open();
            if needs_bind {
                let mut bound = false;
                for _ in 0..16 {
                    let local_port = next_ephemeral(&mut runtime);
                    if runtime
                        .sockets
                        .get_mut::<udp::Socket>(entry.handle)
                        .bind(local_port)
                        .is_ok()
                    {
                        bound = true;
                        break;
                    }
                }
                if !bound {
                    return Err(SocketError::Address);
                }
            }
            runtime.user[id as usize].as_mut().unwrap().peer = Some(endpoint);
            Ok(())
        }
        SocketKind::Tcp => {
            let local_port = next_ephemeral(&mut runtime);
            let Runtime { iface, sockets, .. } = &mut *runtime;
            sockets
                .get_mut::<tcp::Socket>(entry.handle)
                .connect(iface.context(), endpoint, local_port)
                .map_err(|_| SocketError::Address)?;
            runtime.user[id as usize].as_mut().unwrap().peer = Some(endpoint);
            Ok(())
        }
    }
}

pub fn socket_send(owner: u64, id: u64, data: &[u8]) -> Result<usize, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_slot(&runtime, owner, id)?;
    match entry.kind {
        SocketKind::Udp => {
            let peer = entry.peer.ok_or(SocketError::NotConnected)?;
            runtime
                .sockets
                .get_mut::<udp::Socket>(entry.handle)
                .send_slice(data, peer)
                .map(|()| data.len())
                .map_err(|_| SocketError::BufferFull)
        }
        SocketKind::Tcp => runtime
            .sockets
            .get_mut::<tcp::Socket>(entry.handle)
            .send_slice(data)
            .map_err(|_| SocketError::WouldBlock),
    }
}

pub fn socket_recv(
    owner: u64,
    id: u64,
    out: &mut [u8],
) -> Result<(usize, Option<IpEndpoint>), SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_slot(&runtime, owner, id)?;
    match entry.kind {
        SocketKind::Udp => runtime
            .sockets
            .get_mut::<udp::Socket>(entry.handle)
            .recv_slice(out)
            .map(|(len, meta)| (len, Some(meta.endpoint)))
            .map_err(|_| SocketError::WouldBlock),
        SocketKind::Tcp => runtime
            .sockets
            .get_mut::<tcp::Socket>(entry.handle)
            .recv_slice(out)
            .map(|len| (len, entry.peer))
            .map_err(|_| SocketError::WouldBlock),
    }
}

pub fn socket_close(owner: u64, id: u64) -> Result<(), SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let _ = find_slot(&runtime, owner, id)?;
    let index = id as usize;
    if let Some(socket) = runtime.user[index].as_mut() {
        socket.closing = true;
    }
    retire_closed_socket(&mut runtime, index);
    Ok(())
}

pub fn close_process_sockets(owner: u64) {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let mut runtime = runtime.lock();
    for index in 0..MAX_USER_SOCKETS {
        if let Some(entry) = runtime.user[index] {
            if entry.owner == owner {
                if let Some(socket) = runtime.user[index].as_mut() {
                    socket.closing = true;
                }
                retire_closed_socket(&mut runtime, index);
            }
        }
    }
}

pub fn pin_socket(owner: u64, id: u64) -> Result<SocketToken, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_slot(&runtime, owner, id)?;
    let index = usize::try_from(id).map_err(|_| SocketError::Invalid)?;
    let Some(socket) = runtime.user.get_mut(index).and_then(Option::as_mut) else {
        return Err(SocketError::Invalid);
    };
    socket.async_refs = socket
        .async_refs
        .checked_add(1)
        .ok_or(SocketError::BufferFull)?;
    Ok(SocketToken {
        slot: index as u16,
        generation: entry.generation,
        owner,
    })
}

fn find_token(runtime: &Runtime, token: SocketToken) -> Result<UserSocket, SocketError> {
    let socket = runtime
        .user
        .get(token.slot as usize)
        .and_then(|s| *s)
        .ok_or(SocketError::Invalid)?;
    if socket.owner != token.owner || socket.generation != token.generation {
        return Err(SocketError::Invalid);
    }
    Ok(socket)
}

pub fn unpin_socket(token: SocketToken) {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let mut runtime = runtime.lock();
    let index = token.slot as usize;
    let Some(current) = runtime.user.get(index).and_then(|s| *s) else {
        return;
    };
    if current.owner != token.owner || current.generation != token.generation {
        return;
    }
    if let Some(socket) = runtime.user[index].as_mut() {
        if socket.async_refs != 0 {
            socket.async_refs -= 1;
        }
    }
    retire_closed_socket(&mut runtime, index);
}

pub fn socket_connect_pinned(token: SocketToken, endpoint: IpEndpoint) -> Result<(), SocketError> {
    if endpoint.port == 0 {
        return Err(SocketError::Address);
    }
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_token(&runtime, token)?;
    if entry.kind != SocketKind::Tcp {
        return Err(SocketError::WrongKind);
    }

    if let Some(peer) = entry.peer {
        if peer != endpoint {
            return Err(SocketError::Address);
        }
    } else {
        let local_port = next_ephemeral(&mut runtime);
        let Runtime {
            iface,
            sockets,
            user,
            ..
        } = &mut *runtime;
        sockets
            .get_mut::<tcp::Socket>(entry.handle)
            .connect(iface.context(), endpoint, local_port)
            .map_err(|_| SocketError::Address)?;
        if let Some(socket) = user[token.slot as usize].as_mut() {
            socket.peer = Some(endpoint);
        }
    }

    let socket = runtime.sockets.get::<tcp::Socket>(entry.handle);
    if socket.may_send() {
        return Ok(());
    }
    if socket.is_active() {
        return Err(SocketError::WouldBlock);
    }
    Err(SocketError::NotConnected)
}

pub fn socket_send_pinned(token: SocketToken, data: &[u8]) -> Result<usize, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_token(&runtime, token)?;
    match entry.kind {
        SocketKind::Udp => {
            let peer = entry.peer.ok_or(SocketError::NotConnected)?;
            let socket = runtime.sockets.get_mut::<udp::Socket>(entry.handle);
            if !socket.is_open() {
                return Err(SocketError::Address);
            }
            socket
                .send_slice(data, peer)
                .map(|()| data.len())
                .map_err(|_| SocketError::WouldBlock)
        }
        SocketKind::Tcp => {
            let socket = runtime.sockets.get_mut::<tcp::Socket>(entry.handle);
            if !socket.is_active() {
                return Err(SocketError::NotConnected);
            }
            if !socket.may_send() {
                return Err(SocketError::NotConnected);
            }
            if !socket.can_send() {
                return Err(SocketError::WouldBlock);
            }
            socket.send_slice(data).map_err(|_| SocketError::WouldBlock)
        }
    }
}

pub fn socket_recv_pinned(
    token: SocketToken,
    out: &mut [u8],
) -> Result<(usize, Option<IpEndpoint>), SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let entry = find_token(&runtime, token)?;
    match entry.kind {
        SocketKind::Udp => {
            let (len, endpoint) = runtime
                .sockets
                .get_mut::<udp::Socket>(entry.handle)
                .recv_slice(out)
                .map(|(len, meta)| (len, meta.endpoint))
                .map_err(|_| SocketError::WouldBlock)?;
            if let Some(socket) = runtime.user[token.slot as usize].as_mut() {
                socket.peer = Some(endpoint);
            }
            Ok((len, Some(endpoint)))
        }
        SocketKind::Tcp => {
            let socket = runtime.sockets.get_mut::<tcp::Socket>(entry.handle);
            if socket.can_recv() {
                return socket
                    .recv_slice(out)
                    .map(|len| (len, entry.peer))
                    .map_err(|_| SocketError::WouldBlock);
            }
            if !socket.may_recv() {
                return Ok((0, entry.peer));
            }
            Err(SocketError::WouldBlock)
        }
    }
}

pub fn socket_peer(owner: u64, id: u64) -> Result<Option<IpEndpoint>, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let runtime = runtime.lock();
    Ok(find_slot(&runtime, owner, id)?.peer)
}

pub fn net_info() -> NetInfo {
    if let Some(runtime) = RUNTIME.get() {
        let runtime = runtime.lock();
        NetInfo {
            ipv4: runtime.ipv4.octets(),
            gateway: runtime.gateway.octets(),
            dns: runtime.dns_server.octets(),
            prefix: runtime.prefix,
            dhcp_enabled: u8::from(runtime.dhcp_enabled),
            using_dhcp: u8::from(runtime.using_dhcp),
            online: u8::from(virtio_net::is_initialized()),
            mac: virtio_net::mac_address(),
            _reserved: [0; 2],
        }
    } else {
        NetInfo {
            ipv4: [0; 4],
            gateway: [0; 4],
            dns: [0; 4],
            prefix: 0,
            dhcp_enabled: 0,
            using_dhcp: 0,
            online: 0,
            mac: [0; 6],
            _reserved: [0; 2],
        }
    }
}

pub fn dns_start(name: &str) -> Result<u64, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let handle = runtime.dns_handle.ok_or(SocketError::Offline)?;
    let slot = runtime
        .dns_queries
        .iter()
        .position(Option::is_none)
        .ok_or(SocketError::NoSlot)?;
    let query = {
        let Runtime { iface, sockets, .. } = &mut *runtime;
        sockets
            .get_mut::<dns::Socket>(handle)
            .start_query(iface.context(), name, smoltcp::wire::DnsQueryType::A)
            .map_err(|_| SocketError::BufferFull)?
    };
    runtime.dns_queries[slot] = Some(query);
    Ok(slot as u64)
}

pub fn dns_poll(id: u64) -> Result<Option<Ipv4Address>, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let index = usize::try_from(id).map_err(|_| SocketError::Invalid)?;
    let query = runtime
        .dns_queries
        .get(index)
        .and_then(|q| *q)
        .ok_or(SocketError::Invalid)?;
    let handle = runtime.dns_handle.ok_or(SocketError::Offline)?;
    match runtime
        .sockets
        .get_mut::<dns::Socket>(handle)
        .get_query_result(query)
    {
        Ok(addrs) => {
            runtime.dns_queries[index] = None;
            Ok(addrs.into_iter().next().map(|addr| match addr {
                IpAddress::Ipv4(v4) => v4,
            }))
        }
        Err(dns::GetQueryResultError::Pending) => Ok(None),
        Err(_) => {
            runtime.dns_queries[index] = None;
            Err(SocketError::Address)
        }
    }
}

pub fn ping_start(ip: Ipv4Address) -> Result<(), SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    if runtime.ping_pending.is_some() {
        return Err(SocketError::WouldBlock);
    }
    let handle = runtime.ping_handle.ok_or(SocketError::Offline)?;
    runtime.ping_sequence = runtime.ping_sequence.wrapping_add(1);
    let seq = runtime.ping_sequence;
    let mut packet = [0u8; 25];
    packet[0] = 8; // ICMPv4 echo request
    packet[1] = 0;
    packet[4..6].copy_from_slice(&0x5748u16.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[8..].copy_from_slice(b"WovenHatStage5!!!");
    let checksum = internet_checksum(&packet);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    runtime
        .sockets
        .get_mut::<icmp::Socket>(handle)
        .send_slice(&packet, IpAddress::Ipv4(ip))
        .map_err(|_| SocketError::BufferFull)?;
    runtime.ping_pending = Some((ip, seq, timer::ticks()));
    Ok(())
}

/// Returns `Ok(None)` while awaiting a reply and `Ok(Some(rtt_ticks))` once
/// the matching echo reply arrives.
pub fn ping_poll() -> Result<Option<u64>, SocketError> {
    let Some(runtime) = RUNTIME.get() else {
        return Err(SocketError::Offline);
    };
    let mut runtime = runtime.lock();
    let Some((target, seq, started)) = runtime.ping_pending else {
        return Err(SocketError::Invalid);
    };
    if timer::ticks().saturating_sub(started) >= u64::from(timer::FREQUENCY_HZ) * 5 {
        runtime.ping_pending = None;
        return Err(SocketError::WouldBlock);
    }
    let handle = runtime.ping_handle.ok_or(SocketError::Offline)?;
    let mut packet = [0u8; 256];
    match runtime
        .sockets
        .get_mut::<icmp::Socket>(handle)
        .recv_slice(&mut packet)
    {
        Ok((len, source)) => {
            if len >= 8
                && source == IpAddress::Ipv4(target)
                && packet[0] == 0
                && u16::from_be_bytes([packet[4], packet[5]]) == 0x5748
                && u16::from_be_bytes([packet[6], packet[7]]) == seq
            {
                runtime.ping_pending = None;
                Ok(Some(timer::ticks().saturating_sub(started)))
            } else {
                Ok(None)
            }
        }
        Err(_) => Ok(None),
    }
}

fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;
    while i + 1 < data.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32);
        i += 2;
    }
    if i < data.len() {
        sum = sum.wrapping_add((data[i] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn endpoint_from_packed(value: u64) -> Result<IpEndpoint, SocketError> {
    let ip = (value & 0xffff_ffff) as u32;
    let port = ((value >> 32) & 0xffff) as u16;
    if port == 0 {
        return Err(SocketError::Address);
    }
    Ok(IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::from_octets(ip.to_be_bytes())),
        port,
    ))
}

pub fn endpoint_to_packed(endpoint: IpEndpoint) -> u64 {
    let IpAddress::Ipv4(ip) = endpoint.addr;
    u64::from(u32::from_be_bytes(ip.octets())) | ((endpoint.port as u64) << 32)
}

pub fn stats() -> NetStats {
    let Some(runtime) = RUNTIME.get() else {
        return NetStats::default();
    };
    let runtime = runtime.lock();
    NetStats {
        online: virtio_net::is_initialized(),
        echo_active: runtime.echo_handle.is_some(),
        echo_port: runtime.echo_port,
        echo_packets: runtime.echo_packets,
        user_sockets: runtime.user.iter().filter(|s| s.is_some()).count(),
        dhcp_enabled: runtime.dhcp_enabled,
        using_dhcp: runtime.using_dhcp,
    }
}

pub fn initialized() -> bool {
    RUNTIME.get().is_some() && virtio_net::is_initialized()
}

pub fn self_test() -> bool {
    default_cidr().prefix_len() == DEFAULT_PREFIX
        && DEFAULT_GATEWAY == Ipv4Address::new(10, 0, 2, 2)
        && DEFAULT_DNS == Ipv4Address::new(10, 0, 2, 3)
        && virtio_net::self_test()
}

fn now() -> Instant {
    let millis = timer::ticks().saturating_mul(1000) / timer::FREQUENCY_HZ as u64;
    Instant::from_millis(millis as i64)
}

/// Live QEMU/slirp network regression used by the Stage-5 release gate.
///
/// The Python harness supplies host forwards for UDP/7000 and TCP/8080 and
/// waits for the READY markers below before injecting traffic.  Keeping the
/// test in the kernel means DHCP, DNS, ICMP, UDP and TCP are exercised through
/// the same virtio-net/smoltcp runtime used by normal boots rather than through
/// a host-side mock.
#[cfg(any(feature = "qemu-test", feature = "network-test"))]
pub fn qemu_runtime_self_test() -> bool {
    const UDP_PORT: u16 = 7000;
    const TCP_PORT: u16 = 8080;
    const OWNER: u64 = u64::MAX - 0x5748;
    let dhcp_timeout = u64::from(timer::FREQUENCY_HZ) * 8;
    let dns_timeout = u64::from(timer::FREQUENCY_HZ) * 8;
    let ping_timeout = u64::from(timer::FREQUENCY_HZ) * 8;
    let io_timeout = u64::from(timer::FREQUENCY_HZ) * 12;

    if set_dhcp(true).is_err() {
        return false;
    }
    let deadline = timer::ticks().saturating_add(dhcp_timeout);
    while !stats().using_dhcp {
        poll();
        if timer::ticks() >= deadline {
            crate::serial::write_line(format_args!("[NETTEST] DHCP: TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    }
    crate::serial::write_line(format_args!("[NETTEST] DHCP: PASSED"));

    let Ok(query) = dns_start("example.com") else {
        crate::serial::write_line(format_args!("[NETTEST] DNS: START FAILED"));
        return false;
    };
    let deadline = timer::ticks().saturating_add(dns_timeout);
    loop {
        poll();
        match dns_poll(query) {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => {
                crate::serial::write_line(format_args!("[NETTEST] DNS: FAILED"));
                return false;
            }
        }
        if timer::ticks() >= deadline {
            crate::serial::write_line(format_args!("[NETTEST] DNS: TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    }
    crate::serial::write_line(format_args!("[NETTEST] DNS: PASSED"));

    if ping_start(DEFAULT_GATEWAY).is_err() {
        crate::serial::write_line(format_args!("[NETTEST] ICMP: START FAILED"));
        return false;
    }
    let deadline = timer::ticks().saturating_add(ping_timeout);
    loop {
        poll();
        match ping_poll() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => {
                crate::serial::write_line(format_args!("[NETTEST] ICMP: FAILED"));
                return false;
            }
        }
        if timer::ticks() >= deadline {
            crate::serial::write_line(format_args!("[NETTEST] ICMP: TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    }
    crate::serial::write_line(format_args!("[NETTEST] ICMP: PASSED"));

    if start_udp_echo(UDP_PORT).is_err() {
        crate::serial::write_line(format_args!("[NETTEST] UDP: BIND FAILED"));
        return false;
    }
    let initial_packets = stats().echo_packets;
    crate::serial::write_line(format_args!("[NETTEST] UDP READY port={}", UDP_PORT));
    let deadline = timer::ticks().saturating_add(io_timeout);
    while stats().echo_packets == initial_packets {
        poll();
        if timer::ticks() >= deadline {
            crate::serial::write_line(format_args!("[NETTEST] UDP: TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    }
    crate::serial::write_line(format_args!("[NETTEST] UDP: PASSED"));

    let Ok(tcp_id) = socket_open(OWNER, SocketKind::Tcp) else {
        crate::serial::write_line(format_args!("[NETTEST] TCP: OPEN FAILED"));
        return false;
    };
    if socket_bind(OWNER, tcp_id, TCP_PORT).is_err() {
        let _ = socket_close(OWNER, tcp_id);
        crate::serial::write_line(format_args!("[NETTEST] TCP: LISTEN FAILED"));
        return false;
    }
    crate::serial::write_line(format_args!("[NETTEST] TCP READY port={}", TCP_PORT));

    let mut payload = [0u8; 256];
    let deadline = timer::ticks().saturating_add(io_timeout);
    let received = loop {
        poll();
        match socket_recv(OWNER, tcp_id, &mut payload) {
            Ok((len, _)) if len != 0 => break len,
            Ok(_) | Err(SocketError::WouldBlock) => {}
            Err(_) => {
                let _ = socket_close(OWNER, tcp_id);
                crate::serial::write_line(format_args!("[NETTEST] TCP: RECEIVE FAILED"));
                return false;
            }
        }
        if timer::ticks() >= deadline {
            let _ = socket_close(OWNER, tcp_id);
            crate::serial::write_line(format_args!("[NETTEST] TCP: RECEIVE TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    };

    let deadline = timer::ticks().saturating_add(io_timeout);
    loop {
        poll();
        match socket_send(OWNER, tcp_id, &payload[..received]) {
            Ok(len) if len == received => break,
            Ok(_) | Err(SocketError::WouldBlock) | Err(SocketError::BufferFull) => {}
            Err(_) => {
                let _ = socket_close(OWNER, tcp_id);
                crate::serial::write_line(format_args!("[NETTEST] TCP: SEND FAILED"));
                return false;
            }
        }
        if timer::ticks() >= deadline {
            let _ = socket_close(OWNER, tcp_id);
            crate::serial::write_line(format_args!("[NETTEST] TCP: SEND TIMEOUT"));
            return false;
        }
        crate::task::yield_now();
    }
    // Give smoltcp time to put the echoed bytes on the wire before removing the
    // socket.  The host harness verifies the bytes, so this is not just a
    // queueing test.
    for _ in 0..4 {
        poll();
        crate::task::yield_now();
    }
    let _ = socket_close(OWNER, tcp_id);
    crate::serial::write_line(format_args!("[NETTEST] TCP: PASSED"));
    true
}
