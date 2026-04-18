use core::ptr::NonNull;
use spin::Mutex;
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

pub struct RvHal;

unsafe impl Hal for RvHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let paddr = crate::mm::alloc_pages(pages).expect("DMA alloc failed");
        (paddr, NonNull::new(paddr as *mut u8).unwrap())
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        buffer.as_ptr() as *mut u8 as usize
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

pub static NET_DEVICE: Mutex<Option<VirtIONet<RvHal, MmioTransport, 16>>> = Mutex::new(None);

use core::sync::atomic::{AtomicUsize, Ordering};
static MMIO_BASE: AtomicUsize = AtomicUsize::new(0);

/// Manually notify the RX queue to prompt QEMU to check for incoming packets.
pub fn notify_rx() {
    let base = MMIO_BASE.load(Ordering::Relaxed);
    if base != 0 {
        unsafe {
            // Select queue 0 (RX)
            core::ptr::write_volatile((base + 0x30) as *mut u32, 0);
            // Notify queue 0
            core::ptr::write_volatile((base + 0x50) as *mut u32, 0);
        }
    }
}

pub fn init() {
    let addrs = [
        0x1000_1000usize, 0x1000_2000, 0x1000_3000, 0x1000_4000,
        0x1000_5000, 0x1000_6000, 0x1000_7000, 0x1000_8000,
    ];
    for &addr in &addrs {
        let header = match NonNull::new(addr as *mut VirtIOHeader) {
            Some(h) => h,
            None => continue,
        };
        let transport = match unsafe { MmioTransport::new(header) } {
            Ok(t) => t,
            Err(_) => continue,
        };
        if transport.device_type() == DeviceType::Network {
            match VirtIONet::new(transport, 2048) {
                Ok(net) => {
                    let mac = net.mac_address();
                    *NET_DEVICE.lock() = Some(net);
                    MMIO_BASE.store(addr, Ordering::Relaxed);
                    log::info!(
                        "VirtIO-net initialized at {:#x}, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        addr, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    unsafe {
                        let base = addr;
                        core::ptr::write_volatile((base + 0x30) as *mut u32, 0);
                        let q0_num = core::ptr::read_volatile((base + 0x38) as *const u32);
                        let q0_pfn = core::ptr::read_volatile((base + 0x40) as *const u32);
                        let q0_align = core::ptr::read_volatile((base + 0x3c) as *const u32);
                        core::ptr::write_volatile((base + 0x30) as *mut u32, 1);
                        let q1_num = core::ptr::read_volatile((base + 0x38) as *const u32);
                        let q1_pfn = core::ptr::read_volatile((base + 0x40) as *const u32);
                        let q1_align = core::ptr::read_volatile((base + 0x3c) as *const u32);
                        log::info!("Queue0 num={} pfn={:#x} align={}", q0_num, q0_pfn, q0_align);
                        log::info!("Queue1 num={} pfn={:#x} align={}", q1_num, q1_pfn, q1_align);
                        if q0_pfn != 0 {
                            let desc_paddr = (q0_pfn as usize) * 4096;
                            let avail_paddr = desc_paddr + 256;
                            let used_paddr = desc_paddr + 4096;
                            let avail_idx = core::ptr::read_volatile((avail_paddr + 2) as *const u16);
                            let used_idx = core::ptr::read_volatile((used_paddr + 2) as *const u16);
                            log::info!("Queue0 avail_idx={} used_idx={} desc_paddr={:#x}", avail_idx, used_idx, desc_paddr);
                            for i in 0..4 {
                                let daddr = desc_paddr + i * 16;
                                let d0 = core::ptr::read_volatile(daddr as *const u64);
                                let d1 = core::ptr::read_volatile((daddr + 8) as *const u32);
                                let d2 = core::ptr::read_volatile((daddr + 12) as *const u16);
                                let d3 = core::ptr::read_volatile((daddr + 14) as *const u16);
                                log::info!("  desc[{}] addr={:#x} len={} flags={} next={}", i, d0, d1, d2, d3);
                            }
                        }
                    }
                    return;
                }
                Err(e) => {
                    log::warn!("Found virtio-net at {:#x} but failed to initialize: {:?}", addr, e);
                }
            }
        }
    }
    log::warn!("No VirtIO network device found");
}
