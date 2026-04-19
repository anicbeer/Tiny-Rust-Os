use core::arch::asm;
use spin::Mutex;

// Kernel heap allocator (bump allocator)
static HEAP_ALLOCATOR: Mutex<HeapAllocator> = Mutex::new(HeapAllocator::new());

pub struct HeapAllocator {
    current: usize,
    end: usize,
}

impl HeapAllocator {
    pub const fn new() -> Self {
        Self { current: 0, end: 0 }
    }
}

pub fn init_heap() {
    let start = 0x8600_0000;
    let end = 0x8700_0000;
    *HEAP_ALLOCATOR.lock() = HeapAllocator { current: start, end };
}

struct BumpAlloc;

unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let mut alloc = HEAP_ALLOCATOR.lock();
        let addr = (alloc.current + layout.align() - 1) & !(layout.align() - 1);
        if addr + layout.size() <= alloc.end {
            alloc.current = addr + layout.size();
            addr as *mut u8
        } else {
            core::ptr::null_mut()
        }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static GLOBAL_ALLOC: BumpAlloc = BumpAlloc;

extern "C" {
    fn kernel_start();
    fn kernel_end();
}

static PAGE_ALLOCATOR: Mutex<PageAllocator> = Mutex::new(PageAllocator::new());

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

pub struct PageAllocator {
    current: usize,
    end: usize,
}

impl PageAllocator {
    pub const fn new() -> Self {
        Self { current: 0, end: 0 }
    }
}

pub fn init() {
    let kernel_end_addr = kernel_end as usize;
    let start = (kernel_end_addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = 0x8800_0000;
    *PAGE_ALLOCATOR.lock() = PageAllocator { current: start, end };
    log::info!("Page allocator: [{:#x}, {:#x})", start, end);
}

pub fn alloc_page() -> Option<usize> {
    let mut alloc = PAGE_ALLOCATOR.lock();
    if alloc.current + PAGE_SIZE <= alloc.end {
        let page = alloc.current;
        alloc.current += PAGE_SIZE;
        unsafe { core::ptr::write_bytes(page as *mut u8, 0, PAGE_SIZE); }
        Some(page)
    } else {
        None
    }
}

pub fn alloc_pages(n: usize) -> Option<usize> {
    let mut alloc = PAGE_ALLOCATOR.lock();
    let size = n * PAGE_SIZE;
    if alloc.current + size <= alloc.end {
        let page = alloc.current;
        alloc.current += size;
        unsafe { core::ptr::write_bytes(page as *mut u8, 0, size); }
        Some(page)
    } else {
        None
    }
}

pub fn dealloc_page(_addr: usize) {
    // Bump allocator only for now
}

bitflags::bitflags! {
    #[derive(Clone, Copy)]
    pub struct PTEFlags: usize {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

impl PageTableEntry {
    pub fn new(ppn: usize, flags: PTEFlags) -> Self {
        Self { bits: (ppn << 10) | flags.bits() }
    }
    pub fn empty() -> Self {
        Self { bits: 0 }
    }
    pub fn ppn(&self) -> usize {
        self.bits >> 10
    }
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits)
    }
    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }
}

pub struct PageTable {
    root_ppn: usize,
}

impl PageTable {
    pub fn new() -> Option<Self> {
        let root = alloc_page()?;
        unsafe { core::ptr::write_bytes(root as *mut u8, 0, PAGE_SIZE); }
        Some(Self { root_ppn: root >> PAGE_SIZE_BITS })
    }

    pub fn from_ppn(ppn: usize) -> Self {
        Self { root_ppn: ppn }
    }

    pub fn root_ppn(&self) -> usize {
        self.root_ppn
    }

    fn find_pte_create(&mut self, vpn: usize) -> Option<&mut PageTableEntry> {
        let idx = [
            (vpn >> 18) & 0x1ff,
            (vpn >> 9) & 0x1ff,
            vpn & 0x1ff,
        ];
        let mut ppn = self.root_ppn;
        for i in 0..3 {
            let pte = unsafe { &mut *(((ppn << PAGE_SIZE_BITS) as *mut PageTableEntry).add(idx[i])) };
            if i == 2 {
                return Some(pte);
            }
            if !pte.is_valid() {
                let page = alloc_page()?;
                unsafe { core::ptr::write_bytes(page as *mut u8, 0, PAGE_SIZE); }
                *pte = PageTableEntry::new(page >> PAGE_SIZE_BITS, PTEFlags::V);
            }
            ppn = pte.ppn();
        }
        None
    }

    pub fn map(&mut self, va: usize, pa: usize, flags: PTEFlags) {
        let vpn = va >> PAGE_SIZE_BITS;
        let ppn = pa >> PAGE_SIZE_BITS;
        let pte = self.find_pte_create(vpn).expect("out of memory mapping page");
        assert!(!pte.is_valid(), "remap");
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V | PTEFlags::A | PTEFlags::D);
    }

    pub fn unmap(&mut self, va: usize) {
        let vpn = va >> PAGE_SIZE_BITS;
        if let Some(pte) = self.find_pte(vpn) {
            *pte = PageTableEntry::empty();
        }
    }

    pub fn find_pte(&self, vpn: usize) -> Option<&mut PageTableEntry> {
        let idx = [
            (vpn >> 18) & 0x1ff,
            (vpn >> 9) & 0x1ff,
            vpn & 0x1ff,
        ];
        let mut ppn = self.root_ppn;
        for i in 0..3 {
            let pte = unsafe { &mut *(((ppn << PAGE_SIZE_BITS) as *mut PageTableEntry).add(idx[i])) };
            if i == 2 {
                return if pte.is_valid() { Some(pte) } else { None };
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        None
    }

    pub fn translate(&self, va: usize) -> Option<usize> {
        let vpn = va >> PAGE_SIZE_BITS;
        let offset = va & (PAGE_SIZE - 1);
        self.find_pte(vpn).map(|pte| {
            (pte.ppn() << PAGE_SIZE_BITS) | offset
        })
    }

    pub fn dump_pte(&self, va: usize) {
        let vpn = va >> PAGE_SIZE_BITS;
        let idx = [
            (vpn >> 18) & 0x1ff,
            (vpn >> 9) & 0x1ff,
            vpn & 0x1ff,
        ];
        let mut ppn = self.root_ppn;
        for i in 0..3 {
            let pte_addr = (ppn << PAGE_SIZE_BITS) + idx[i] * core::mem::size_of::<PageTableEntry>();
            let pte = unsafe { &*(pte_addr as *const PageTableEntry) };
            log::info!("  level{}: idx={} pte_addr={:#x} pte={:#x} valid={}", i, idx[i], pte_addr, pte.bits, pte.is_valid());
            if i == 2 {
                return;
            }
            if !pte.is_valid() {
                log::info!("  -> not valid, stop");
                return;
            }
            ppn = pte.ppn();
        }
    }

    /// Clone the page table, copying all user pages (pages with U flag) to new physical pages.
    /// Kernel identity mappings are recreated without copying.
    pub fn clone_user_space(&self) -> Option<Self> {
        let mut new_pt = PageTable::new()?;
        // Identity-map kernel and device regions
        for addr in (0x8000_0000..0x8800_0000).step_by(PAGE_SIZE) {
            new_pt.map(addr, addr, PTEFlags::R | PTEFlags::W | PTEFlags::X);
        }
        new_pt.map(0x1000_0000, 0x1000_0000, PTEFlags::R | PTEFlags::W);
        for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
            new_pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
        }
        for addr in (0x0c00_0000..0x0c20_0000).step_by(PAGE_SIZE) {
            new_pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
        }

        // Walk the page table and copy user pages
        let root = self.root_ppn << PAGE_SIZE_BITS;
        for i in 0..512 {
            let pte0 = unsafe { &*(root as *const PageTableEntry).add(i) };
            if !pte0.is_valid() { continue; }
            let ppn0 = pte0.ppn();
            let addr0 = ppn0 << PAGE_SIZE_BITS;
            for j in 0..512 {
                let pte1 = unsafe { &*(addr0 as *const PageTableEntry).add(j) };
                if !pte1.is_valid() { continue; }
                let ppn1 = pte1.ppn();
                let addr1 = ppn1 << PAGE_SIZE_BITS;
                for k in 0..512 {
                    let pte2 = unsafe { &*(addr1 as *const PageTableEntry).add(k) };
                    if !pte2.is_valid() { continue; }
                    let flags = pte2.flags();
                    if !flags.contains(PTEFlags::U) { continue; }

                    let vpn = (i << 18) | (j << 9) | k;
                    let va = vpn << PAGE_SIZE_BITS;
                    let old_pa = pte2.ppn() << PAGE_SIZE_BITS;

                    let new_pa = alloc_page()?;
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            old_pa as *const u8,
                            new_pa as *mut u8,
                            PAGE_SIZE,
                        );
                    }
                    new_pt.map(va, new_pa, flags);
                }
            }
        }
        Some(new_pt)
    }
}

pub static KERNEL_PAGE_TABLE: Mutex<Option<PageTable>> = Mutex::new(None);

const KERNEL_BASE: usize = 0x8020_0000;

pub fn init_kernel_page_table() {
    let mut pt = PageTable::new().unwrap();
    // Identity map kernel and physical memory
    let start = 0x8000_0000usize;
    let end = 0x8800_0000usize;
    for addr in (start..end).step_by(PAGE_SIZE) {
        pt.map(addr, addr, PTEFlags::R | PTEFlags::W | PTEFlags::X);
    }
    // Also map UART
    pt.map(0x1000_0000, 0x1000_0000, PTEFlags::R | PTEFlags::W);
    // virtio mmio regions (0x10001000 - 0x10008000 approx)
    for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
        pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
    }
    // PLIC
    for addr in (0x0c00_0000..0x0c20_0000).step_by(PAGE_SIZE) {
        pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
    }
    pt.map(0x100000, 0x100000, PTEFlags::R | PTEFlags::W);

    let satp = (8usize << 60) | pt.root_ppn();
    *KERNEL_PAGE_TABLE.lock() = Some(pt);
    unsafe {
        asm!("csrw satp, {}", in(reg) satp);
        asm!("sfence.vma");
    }
    log::info!("SV39 page table enabled");
}
