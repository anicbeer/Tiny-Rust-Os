use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::mem::MaybeUninit;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address,
};
use virtio_drivers::device::net::RxBuffer;

pub struct NetDevice;

impl Device for NetDevice {
    type RxToken<'a> = NetRxToken;
    type TxToken<'a> = NetTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut net = crate::virtio::NET_DEVICE.lock();
        if let Some(ref mut n) = *net {
            match n.receive() {
                Ok(rx_buf) => {
                    let len = rx_buf.packet().len();
                    drop(net);
                    log::info!("NetDevice::receive got packet len={}", len);
                    return Some((NetRxToken { buffer: Some(rx_buf) }, NetTxToken));
                }
                Err(_) => {
                    drop(net);
                    crate::virtio::notify_rx();
                }
            }
        }
        None
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let net = crate::virtio::NET_DEVICE.lock();
        if let Some(ref n) = *net {
            if n.can_send() {
                return Some(NetTxToken);
            }
        }
        None
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct NetRxToken {
    buffer: Option<RxBuffer>,
}

impl RxToken for NetRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = self.buffer.unwrap();
        let result = f(buffer.packet_mut());
        if let Some(ref mut net) = *crate::virtio::NET_DEVICE.lock() {
            let _ = net.recycle_rx_buffer(buffer);
        }
        result
    }
}

pub struct NetTxToken;

impl TxToken for NetTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        if let Some(ref mut net) = *crate::virtio::NET_DEVICE.lock() {
            let mut tx_buf = net.new_tx_buffer(len);
            let result = f(tx_buf.packet_mut());
            let _ = net.send(tx_buf);
            return result;
        }
        let mut dummy = alloc::vec![0u8; len];
        f(&mut dummy)
    }
}

static NET_IFACE: spin::Mutex<MaybeUninit<Interface>> = spin::Mutex::new(MaybeUninit::uninit());
static NET_SOCKETS: spin::Mutex<MaybeUninit<SocketSet<'static>>> = spin::Mutex::new(MaybeUninit::uninit());
static FD_TO_HANDLE: spin::Mutex<BTreeMap<usize, SocketHandle>> = spin::Mutex::new(BTreeMap::new());
static EPOLL_DATA: spin::Mutex<BTreeMap<usize, u64>> = spin::Mutex::new(BTreeMap::new());
static LISTEN_FD: spin::Mutex<Option<usize>> = spin::Mutex::new(None);

pub fn get_time_ms() -> i64 {
    let time: usize;
    unsafe {
        core::arch::asm!("rdtime {}", out(reg) time);
    }
    (time / 10_000) as i64
}

pub fn init() {
    if crate::virtio::NET_DEVICE.lock().is_none() {
        log::warn!("Network stack init skipped: no virtio-net device");
        return;
    }

    let mac = {
        let net = crate::virtio::NET_DEVICE.lock();
        net.as_ref().map(|n| n.mac_address()).unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
    };

    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress::from_bytes(&mac)));
    let iface = Interface::new(config, &mut NetDevice, Instant::from_millis(get_time_ms()));
    *NET_IFACE.lock() = MaybeUninit::new(iface);

    let mut storage = alloc::vec::Vec::with_capacity(8);
    for _ in 0..8 { storage.push(SocketStorage::EMPTY); }
    let sockets = SocketSet::new(Vec::leak(storage));
    *NET_SOCKETS.lock() = MaybeUninit::new(sockets);

    unsafe {
        let iface = &mut *NET_IFACE.lock().as_mut_ptr();
        iface.update_ip_addrs(|ip_addrs| {
            ip_addrs.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)).unwrap();
        });
        iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2)).unwrap();
    }

    log::info!(
        "Network stack initialized, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // Test: send a raw ARP request to trigger QEMU/SLIRP response
    {
        let mut net = crate::virtio::NET_DEVICE.lock();
        if let Some(ref mut n) = *net {
            let mut tx_buf = n.new_tx_buffer(42);
            let pkt = tx_buf.packet_mut();
            // Ethernet header
            pkt[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
            pkt[6..12].copy_from_slice(&mac);
            pkt[12..14].copy_from_slice(&[0x08, 0x06]);
            // ARP request
            pkt[14..16].copy_from_slice(&[0x00, 0x01]);
            pkt[16..18].copy_from_slice(&[0x08, 0x00]);
            pkt[18] = 6;
            pkt[19] = 4;
            pkt[20..22].copy_from_slice(&[0x00, 0x01]);
            pkt[22..28].copy_from_slice(&mac);
            pkt[28..32].copy_from_slice(&[10, 0, 2, 15]);
            pkt[32..38].copy_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
            pkt[38..42].copy_from_slice(&[10, 0, 2, 2]);
            log::info!("Sending ARP request...");
            if let Err(e) = n.send(tx_buf) {
                log::warn!("ARP send failed: {:?}", e);
            } else {
                log::info!("ARP request sent");
            }
        }
    }
    // Poll for ARP response via smoltcp so it populates ARP cache
    for _ in 0..500 {
        poll_network();
        for _ in 0..5000 { core::hint::spin_loop(); }
    }
    log::info!("ARP poll done");
}

fn create_listen_socket() -> Option<SocketHandle> {
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        let tcp_socket = TcpSocket::new(
            SocketBuffer::new(Vec::leak(alloc::vec![0u8; 4096])),
            SocketBuffer::new(Vec::leak(alloc::vec![0u8; 4096])),
        );
        let handle = sockets.add(tcp_socket);
        let socket = sockets.get_mut::<TcpSocket>(handle);
        socket.listen(80).ok()?;
        Some(handle)
    }
}

pub fn bind_listen_fd(fd: usize) {
    let already = LISTEN_FD.lock().as_ref().copied() == Some(fd);
    if already {
        return;
    }
    if let Some(handle) = create_listen_socket() {
        FD_TO_HANDLE.lock().insert(fd, handle);
        *LISTEN_FD.lock() = Some(fd);
        log::info!("Listening socket bound to fd {}", fd);
    }
}

pub fn set_epoll_data(fd: usize, data: u64) {
    EPOLL_DATA.lock().insert(fd, data);
}

pub fn poll_network() {
    {
        let net = crate::virtio::NET_DEVICE.lock();
        if net.is_none() {
            return;
        }
    }
    let timestamp = Instant::from_millis(get_time_ms());
    let mut device = NetDevice;
    unsafe {
        let iface = &mut *NET_IFACE.lock().as_mut_ptr();
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        let changed = iface.poll(timestamp, &mut device, sockets);
        if changed {
            log::info!("poll_network: state changed");
        }
    }
}

pub fn is_listen_readable() -> bool {
    let fd = match LISTEN_FD.lock().as_ref().copied() {
        Some(f) => f,
        None => return false,
    };
    poll_network();
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        if let Some(&handle) = FD_TO_HANDLE.lock().get(&fd) {
            let socket = sockets.get_mut::<TcpSocket>(handle);
            return !socket.is_listening() && socket.is_open();
        }
    }
    false
}

pub fn accept_connection(fd: usize) -> Option<SocketHandle> {
    let listen_fd = LISTEN_FD.lock().as_ref().copied()?;
    if fd != listen_fd {
        return None;
    }
    poll_network();
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        let handle = *FD_TO_HANDLE.lock().get(&listen_fd)?;
        let socket = sockets.get_mut::<TcpSocket>(handle);
        if socket.is_listening() {
            return None;
        }
        // Connection established. Replace listen socket and return old handle.
        let new_handle = create_listen_socket()?;
        log::info!("accept_connection: returning handle={:?} for fd={}, new_listen={:?}", handle, fd, new_handle);
        FD_TO_HANDLE.lock().insert(listen_fd, new_handle);
        Some(handle)
    }
}

pub fn recv_from_fd(fd: usize, buf: &mut [u8]) -> isize {
    poll_network();
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        if let Some(&handle) = FD_TO_HANDLE.lock().get(&fd) {
            let socket = sockets.get_mut::<TcpSocket>(handle);
            if !socket.can_recv() {
                if socket.is_open() {
                    return -11; // EAGAIN
                } else {
                    return 0; // EOF
                }
            }
            return socket.recv(|data| {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                (len, len)
            }).unwrap_or(0) as isize;
        }
    }
    -1
}

pub fn send_to_fd(fd: usize, buf: &[u8]) -> isize {
    poll_network();
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        if let Some(&handle) = FD_TO_HANDLE.lock().get(&fd) {
            let socket = sockets.get_mut::<TcpSocket>(handle);
            if !socket.can_send() {
                if socket.is_open() {
                    return -11; // EAGAIN
                } else {
                    return -32; // EPIPE
                }
            }
            let to_write = buf.len();
            return socket.send(|data| {
                let len = data.len().min(to_write);
                data[..len].copy_from_slice(&buf[..len]);
                (len, len)
            }).unwrap_or(0) as isize;
        }
    }
    -1
}

pub fn close_fd(fd: usize) {
    let is_listen = {
        let listen_fd = LISTEN_FD.lock();
        matches!(*listen_fd, Some(lfd) if lfd == fd)
    };
    if is_listen {
        EPOLL_DATA.lock().remove(&fd);
        return;
    }
    unsafe {
        let mut fd_map = FD_TO_HANDLE.lock();
        if let Some(handle) = fd_map.remove(&fd) {
            let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
            let _ = sockets.remove(handle);
        }
        EPOLL_DATA.lock().remove(&fd);
    }
}

pub fn add_fd_handle(fd: usize, handle: SocketHandle) {
    FD_TO_HANDLE.lock().insert(fd, handle);
}

pub fn is_socket_fd(fd: usize) -> bool {
    FD_TO_HANDLE.lock().contains_key(&fd)
}

pub fn get_epoll_events(_epfd: isize, events_buf: *mut u8, maxevents: usize) -> usize {
    poll_network();
    let mut count = 0;
    unsafe {
        let sockets = &mut *NET_SOCKETS.lock().as_mut_ptr();
        let fd_map = FD_TO_HANDLE.lock();
        let epoll_data = EPOLL_DATA.lock();
        for (&fd, &handle) in fd_map.iter() {
            if count >= maxevents {
                break;
            }
            let socket = sockets.get_mut::<TcpSocket>(handle);
            let mut revents: u32 = 0;
            if socket.can_recv() || (!socket.is_listening() && socket.is_open()) {
                revents |= 0x001; // EPOLLIN
            }
            if socket.can_send() {
                revents |= 0x004; // EPOLLOUT
            }
            if revents != 0 {
                // struct epoll_event on riscv64 Linux: u32 events; u32 padding; u64 data;
                // Total size = 16 bytes.
                let evt = events_buf.add(count * 16) as *mut u32;
                *evt = revents;
                let data = epoll_data.get(&fd).copied().unwrap_or(fd as u64);
                core::ptr::write_unaligned(evt.add(2) as *mut u64, data);
                count += 1;
            }
        }
    }
    count
}
