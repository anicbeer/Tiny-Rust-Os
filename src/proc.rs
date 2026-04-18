use core::arch::asm;
use spin::Mutex;
use crate::mm::{PageTable, PTEFlags, PAGE_SIZE, alloc_pages};
use crate::trap::TrapFrame;

pub struct Process {
    pub pid: usize,
    pub page_table: PageTable,
    pub trap_frame: TrapFrame,
    pub kernel_stack: usize,
    pub brk: usize,
}

impl Process {
    pub fn new(pid: usize) -> Option<Self> {
        let mut pt = PageTable::new()?;
        let kstack = alloc_pages(2)?; // 8KB kernel stack
        // Identity-map kernel and device regions so traps from U-mode can run the handler
        for addr in (0x8000_0000..0x8800_0000).step_by(PAGE_SIZE) {
            pt.map(addr, addr, PTEFlags::R | PTEFlags::W | PTEFlags::X);
        }
        pt.map(0x1000_0000, 0x1000_0000, PTEFlags::R | PTEFlags::W);
        for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
            pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
        }
        for addr in (0x0c00_0000..0x0c20_0000).step_by(PAGE_SIZE) {
            pt.map(addr, addr, PTEFlags::R | PTEFlags::W);
        }
        Some(Self {
            pid,
            page_table: pt,
            trap_frame: TrapFrame::new(),
            kernel_stack: kstack + 2 * PAGE_SIZE,
            brk: 0,
        })
    }
}

pub static CURRENT_PROC: Mutex<Option<Process>> = Mutex::new(None);

pub fn init_user_proc(elf_data: &[u8]) -> Option<()> {
    let mut proc = Process::new(1)?;

    // Map user stack at top of lower half (include the top page for argc/argv)
    let stack_top = 0x0000_003f_ffff_f000usize;
    let stack_pages = 8;
    let mut top_page_paddr = 0;
    for i in 0..=stack_pages {
        let page = crate::mm::alloc_page()?;
        if i == 0 { top_page_paddr = page; }
        proc.page_table.map(stack_top - i * PAGE_SIZE, page, PTEFlags::U | PTEFlags::R | PTEFlags::W);
    }
    // Write minimal initial stack: argc, argv[0], envp
    unsafe {
        let ptr = top_page_paddr as *mut usize;
        ptr.add(0).write_volatile(1);                 // argc = 1
        ptr.add(1).write_volatile(stack_top + 32);    // argv[0] pointer
        ptr.add(2).write_volatile(0);                 // argv terminator
        ptr.add(3).write_volatile(0);                 // envp terminator
        core::ptr::copy_nonoverlapping(b"nginx\0".as_ptr(), (top_page_paddr + 32) as *mut u8, 6);
    }

    // Copy ELF to aligned buffer for safe parsing
    let layout = alloc::alloc::Layout::from_size_align(elf_data.len(), 16).ok()?;
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() { return None; }
    unsafe { core::ptr::copy_nonoverlapping(elf_data.as_ptr(), ptr, elf_data.len()); }
    let aligned_elf = unsafe { core::slice::from_raw_parts(ptr, elf_data.len()) };

    // Parse ELF header for auxv info
    let elf = xmas_elf::ElfFile::new(aligned_elf).ok()?;
    let header = elf.header;
    let entry = header.pt2.entry_point() as usize;
    let phdr_offset = header.pt2.ph_offset() as usize;
    let phent = header.pt2.ph_entry_size() as usize;
    let phnum = header.pt2.ph_count() as usize;
    let base_vaddr = elf.program_iter()
        .filter(|ph| ph.get_type().ok() == Some(xmas_elf::program::Type::Load))
        .map(|ph| ph.virtual_addr() as usize)
        .min()
        .unwrap_or(0);
    let at_phdr = base_vaddr + phdr_offset;

    // Write minimal initial stack: argc, argv, envp, auxv
    unsafe {
        let base = top_page_paddr;
        let mut off = 0usize;

        // argc = 1
        (base as *mut usize).write_volatile(1);
        off += 8;

        // argv[0] pointer
        ((base + off) as *mut usize).write_volatile(stack_top + 0x100);
        off += 8;

        // argv terminator
        ((base + off) as *mut usize).write_volatile(0);
        off += 8;

        // envp terminator
        ((base + off) as *mut usize).write_volatile(0);
        off += 8;

        // Auxv
        let aux_base = base + off;
        let mut aoff = 0usize;
        macro_rules! push_aux {
            ($t:expr, $v:expr) => {
                ((aux_base + aoff) as *mut usize).write_volatile($t);
                ((aux_base + aoff + 8) as *mut usize).write_volatile($v);
                aoff += 16;
            };
        }
        push_aux!(6, 4096);               // AT_PAGESZ
        push_aux!(3, at_phdr);            // AT_PHDR
        push_aux!(4, phent);              // AT_PHENT
        push_aux!(5, phnum);              // AT_PHNUM
        push_aux!(9, entry);              // AT_ENTRY
        push_aux!(25, stack_top + 0x200); // AT_RANDOM
        push_aux!(17, 100);               // AT_CLKTCK
        push_aux!(0, 0);                  // AT_NULL
        off += aoff;

        // argv[0] string "nginx\0"
        core::ptr::copy_nonoverlapping(b"nginx\0".as_ptr(), (base + 0x100) as *mut u8, 6);

        // random bytes for AT_RANDOM
        core::ptr::write_bytes((base + 0x200) as *mut u8, 0xAB, 16);
    }

    let max_end = load_elf(&mut proc, aligned_elf, &elf)?;
    proc.brk = (max_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if proc.brk == 0 {
        proc.brk = 0x10000; // fallback minimum
    }
    {
        proc.trap_frame.sepc = entry;
        proc.trap_frame.sstatus = (1 << 5) | (1 << 13); // SPIE=1, SPP=0, FS=Initial
        proc.trap_frame.regs[2] = stack_top; // sp
        *CURRENT_PROC.lock() = Some(proc);
        Some(())
    }
}

fn load_elf(proc: &mut Process, data: &[u8], elf: &xmas_elf::ElfFile) -> Option<usize> {
    use xmas_elf::program::Type;

    let mut max_end = 0usize;

    for ph in elf.program_iter() {
        if ph.get_type().ok()? == Type::Load {
            let vaddr = ph.virtual_addr() as usize;
            let memsz = ph.mem_size() as usize;
            let filesz = ph.file_size() as usize;
            let offset = ph.offset() as usize;
            let flags = ph.flags();

            let mut pte_flags = PTEFlags::U;
            if flags.is_read() { pte_flags |= PTEFlags::R; }
            if flags.is_write() { pte_flags |= PTEFlags::W; }
            if flags.is_execute() { pte_flags |= PTEFlags::X; }

            let seg_begin = vaddr;
            let seg_file_end = vaddr.saturating_add(filesz);
            let seg_mem_end = vaddr.saturating_add(memsz);
            if seg_mem_end > max_end {
                max_end = seg_mem_end;
            }
            let first_page = vaddr & !(PAGE_SIZE - 1);
            let last_page = (seg_mem_end - 1) & !(PAGE_SIZE - 1);
            let mut page = first_page;

            while page <= last_page {
                let paddr = crate::mm::alloc_page()?;
                proc.page_table.map(page, paddr, pte_flags);

                let page_mem_end = (page + PAGE_SIZE).min(seg_mem_end);
                let zero_len = page_mem_end - page;
                unsafe { core::ptr::write_bytes(paddr as *mut u8, 0, zero_len); }

                let page_file_start = page.max(seg_begin);
                let page_file_end = (page + PAGE_SIZE).min(seg_file_end);
                if page_file_end > page_file_start {
                    let copy_offset = offset + (page_file_start - seg_begin);
                    let dest_offset = page_file_start - page;
                    let copy_len = page_file_end - page_file_start;
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data.as_ptr().add(copy_offset),
                            (paddr + dest_offset) as *mut u8,
                            copy_len,
                        );
                    }
                }

                page += PAGE_SIZE;
            }
        }
    }

    Some(max_end)
}

pub fn run_user() -> ! {
    let (satp, sepc, sstatus, sp, ksp) = {
        let proc = CURRENT_PROC.lock();
        let p = proc.as_ref().expect("no user process");
        let satp = (8usize << 60) | p.page_table.root_ppn();
        (satp, p.trap_frame.sepc, p.trap_frame.sstatus, p.trap_frame.regs[2], p.kernel_stack)
    };

    log::info!("Entering user mode: sepc={:#x} sp={:#x} satp={:#x} sstatus={:#x}", sepc, sp, satp, sstatus);

    unsafe {
        asm!(
            "csrw sscratch, {ksp}",
            "csrw satp, {satp}",
            "sfence.vma",
            "csrw sepc, {sepc}",
            "csrw sstatus, {sstatus}",
            "mv sp, {usp}",
            "sret",
            ksp = in(reg) ksp,
            satp = in(reg) satp,
            sepc = in(reg) sepc,
            sstatus = in(reg) sstatus,
            usp = in(reg) sp,
            options(noreturn)
        );
    }
}
