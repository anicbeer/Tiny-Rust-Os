use core::arch::asm;
use spin::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use crate::mm::{PageTable, PTEFlags, PAGE_SIZE, alloc_pages};
use crate::trap::TrapFrame;
use crate::syscall::FdTable;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ProcState {
    Ready,
    Running,
    Zombie,
}

pub struct Process {
    pub pid: usize,
    pub ppid: usize,
    pub state: ProcState,
    pub page_table: PageTable,
    pub trap_frame: TrapFrame,
    pub kernel_stack: usize,
    pub brk: usize,
    pub exit_code: isize,
    pub fd_table: FdTable,
}

unsafe impl Send for Process {}

pub static PROC_TABLE: Mutex<BTreeMap<usize, Process>> = Mutex::new(BTreeMap::new());
pub static CURRENT_PID: Mutex<usize> = Mutex::new(0);
static NEXT_PID: Mutex<usize> = Mutex::new(1);

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
            ppid: 0,
            state: ProcState::Ready,
            page_table: pt,
            trap_frame: TrapFrame::new(),
            kernel_stack: kstack + 2 * PAGE_SIZE,
            brk: 0,
            exit_code: 0,
            fd_table: FdTable::new(),
        })
    }
}

pub fn init_user_proc(elf_data: &[u8], argv: &[&str]) -> Option<()> {
    let mut proc = Process::new(1)?;
    let stack_top = 0x0000_003f_ffff_f000usize;
    let mut top_page_paddr = 0;
    for i in 0..=8 {
        let page = crate::mm::alloc_page()?;
        if i == 0 { top_page_paddr = page; }
        proc.page_table.map(stack_top - i * PAGE_SIZE, page, PTEFlags::U | PTEFlags::R | PTEFlags::W);
    }

    let aligned_elf = align_elf(elf_data)?;
    let elf = xmas_elf::ElfFile::new(aligned_elf).ok()?;
    let entry = elf.header.pt2.entry_point() as usize;
    let phdr_offset = elf.header.pt2.ph_offset() as usize;
    let phent = elf.header.pt2.ph_entry_size() as usize;
    let phnum = elf.header.pt2.ph_count() as usize;
    let base_vaddr = elf.program_iter()
        .filter(|ph| ph.get_type().ok() == Some(xmas_elf::program::Type::Load))
        .map(|ph| ph.virtual_addr() as usize)
        .min()
        .unwrap_or(0);
    let at_phdr = base_vaddr + phdr_offset;

    let max_end = load_elf_into(&mut proc.page_table, aligned_elf, &elf)?;
    let argv_owned: alloc::vec::Vec<alloc::string::String> = argv.iter().map(|s| s.to_string()).collect();
    let envp_owned = alloc::vec![alloc::string::String::from("PATH=/bin")];
    let sp = build_stack(top_page_paddr, stack_top, &argv_owned, &envp_owned, at_phdr, phent, phnum, entry);

    proc.brk = (max_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if proc.brk == 0 { proc.brk = 0x10000; }
    proc.trap_frame.sepc = entry;
    proc.trap_frame.sstatus = (1 << 5) | (1 << 13);
    proc.trap_frame.regs[2] = sp;
    proc.trap_frame.regs[10] = argv.len();
    proc.state = ProcState::Running;

    *CURRENT_PID.lock() = 1;
    PROC_TABLE.lock().insert(1, proc);
    *NEXT_PID.lock() = 2;
    Some(())
}

fn load_elf_into(pt: &mut PageTable, data: &[u8], elf: &xmas_elf::ElfFile) -> Option<usize> {
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
            if seg_mem_end > max_end { max_end = seg_mem_end; }
            let first_page = vaddr & !(PAGE_SIZE - 1);
            let last_page = (seg_mem_end - 1) & !(PAGE_SIZE - 1);
            let mut page = first_page;
            while page <= last_page {
                let paddr = crate::mm::alloc_page()?;
                pt.map(page, paddr, pte_flags);
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

fn load_elf(proc: &mut Process, data: &[u8], elf: &xmas_elf::ElfFile) -> Option<usize> {
    load_elf_into(&mut proc.page_table, data, elf)
}

fn align_elf(elf_data: &[u8]) -> Option<&[u8]> {
    let layout = alloc::alloc::Layout::from_size_align(elf_data.len(), 16).ok()?;
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() { return None; }
    unsafe { core::ptr::copy_nonoverlapping(elf_data.as_ptr(), ptr, elf_data.len()); }
    Some(unsafe { core::slice::from_raw_parts(ptr, elf_data.len()) })
}

fn build_stack(
    top_page_paddr: usize,
    stack_top: usize,
    argv: &[alloc::string::String],
    envp: &[alloc::string::String],
    at_phdr: usize,
    phent: usize,
    phnum: usize,
    entry: usize,
) -> usize {
    unsafe {
        let base = top_page_paddr;
        let mut off = 0usize;
        // argc
        (base as *mut usize).write_volatile(argv.len());
        off += 8;
        // argv pointers
        let _argv_ptr_base = stack_top + off;
        for (i, _s) in argv.iter().enumerate() {
            ((base + off) as *mut usize).write_volatile(stack_top + 0x400 + i * 0x100);
            off += 8;
        }
        ((base + off) as *mut usize).write_volatile(0);
        off += 8;
        // envp pointers
        let _envp_ptr_base = stack_top + off;
        for (i, _s) in envp.iter().enumerate() {
            ((base + off) as *mut usize).write_volatile(stack_top + 0x400 + argv.len() * 0x100 + i * 0x100);
            off += 8;
        }
        ((base + off) as *mut usize).write_volatile(0);
        off += 8;
        // auxv
        let aux_base = base + off;
        let mut aoff = 0usize;
        macro_rules! push_aux {
            ($t:expr, $v:expr) => {
                ((aux_base + aoff) as *mut usize).write_volatile($t);
                ((aux_base + aoff + 8) as *mut usize).write_volatile($v);
                aoff += 16;
            };
        }
        push_aux!(6, 4096);
        push_aux!(3, at_phdr);
        push_aux!(4, phent);
        push_aux!(5, phnum);
        push_aux!(9, entry);
        push_aux!(25, stack_top + 0x200);
        push_aux!(17, 100);
        push_aux!(0, 0);
        let _ = aoff;

        // argv strings
        let mut str_off = 0x400usize;
        for s in argv.iter() {
            let len = s.len();
            core::ptr::copy_nonoverlapping(s.as_ptr(), (base + str_off) as *mut u8, len);
            *((base + str_off + len) as *mut u8) = 0;
            str_off += 0x100;
        }
        // envp strings
        for s in envp.iter() {
            let len = s.len();
            core::ptr::copy_nonoverlapping(s.as_ptr(), (base + str_off) as *mut u8, len);
            *((base + str_off + len) as *mut u8) = 0;
            str_off += 0x100;
        }
        // random bytes
        core::ptr::write_bytes((base + 0x200) as *mut u8, 0xAB, 16);

        stack_top
    }
}

/// Replace current process with a new ELF binary. Returns new user sp.
pub fn exec_process(proc: &mut Process, elf_data: &[u8], argv: &[alloc::string::String], envp: &[alloc::string::String]) -> Option<usize> {
    let mut new_pt = PageTable::new()?;
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

    let stack_top = 0x0000_003f_ffff_f000usize;
    let mut top_page_paddr = 0;
    for i in 0..=8 {
        let page = crate::mm::alloc_page()?;
        if i == 0 { top_page_paddr = page; }
        new_pt.map(stack_top - i * PAGE_SIZE, page, PTEFlags::U | PTEFlags::R | PTEFlags::W);
    }

    let aligned_elf = align_elf(elf_data)?;
    let elf = xmas_elf::ElfFile::new(aligned_elf).ok()?;
    let entry = elf.header.pt2.entry_point() as usize;
    let phdr_offset = elf.header.pt2.ph_offset() as usize;
    let phent = elf.header.pt2.ph_entry_size() as usize;
    let phnum = elf.header.pt2.ph_count() as usize;
    let base_vaddr = elf.program_iter()
        .filter(|ph| ph.get_type().ok() == Some(xmas_elf::program::Type::Load))
        .map(|ph| ph.virtual_addr() as usize)
        .min()
        .unwrap_or(0);
    let at_phdr = base_vaddr + phdr_offset;

    let max_end = load_elf_into(&mut new_pt, aligned_elf, &elf)?;
    let sp = build_stack(top_page_paddr, stack_top, argv, envp, at_phdr, phent, phnum, entry);

    proc.page_table = new_pt;
    proc.brk = (max_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if proc.brk == 0 { proc.brk = 0x10000; }
    proc.trap_frame.sepc = entry;
    proc.trap_frame.sstatus = (1 << 5) | (1 << 13);
    proc.trap_frame.regs[2] = sp;

    Some(sp)
}

/// Fork the current process. Returns child pid on success.
pub fn fork_process(parent_tf: &TrapFrame) -> Option<usize> {
    let parent_pid = *CURRENT_PID.lock();
    let child_pt;
    let child_kstack;
    let child_brk;
    let child_fd_table;

    {
        let table = PROC_TABLE.lock();
        let parent = table.get(&parent_pid)?;
        child_pt = parent.page_table.clone_user_space()?;
        child_kstack = alloc_pages(2)?;
        child_brk = parent.brk;
        child_fd_table = parent.fd_table.clone();
    }

    let mut child_tf = parent_tf.clone();
    child_tf.regs[10] = 0; // a0 = 0 for child
    child_tf.sepc += 4;   // resume after ecall instruction

    let mut next_pid = NEXT_PID.lock();
    let pid = *next_pid;
    *next_pid += 1;

    let child = Process {
        pid,
        ppid: parent_pid,
        state: ProcState::Ready,
        page_table: child_pt,
        trap_frame: child_tf,
        kernel_stack: child_kstack + 2 * PAGE_SIZE,
        brk: child_brk,
        exit_code: 0,
        fd_table: child_fd_table,
    };

    PROC_TABLE.lock().insert(pid, child);
    Some(pid)
}

/// Mark current process as zombie and store exit code.
pub fn exit_process(code: isize) {
    let pid = *CURRENT_PID.lock();
    let mut table = PROC_TABLE.lock();
    if let Some(proc) = table.get_mut(&pid) {
        proc.state = ProcState::Zombie;
        proc.exit_code = code;
        log::info!("Process {} exited with code {}", pid, code);
    }
}

/// Get a reference to the current process's trap frame.
pub fn current_trap_frame() -> Option<TrapFrame> {
    let pid = *CURRENT_PID.lock();
    PROC_TABLE.lock().get(&pid).map(|p| p.trap_frame.clone())
}

/// Set the current process's trap frame.
pub fn set_current_trap_frame(tf: &TrapFrame) {
    let pid = *CURRENT_PID.lock();
    let mut table = PROC_TABLE.lock();
    if let Some(proc) = table.get_mut(&pid) {
        proc.trap_frame = tf.clone();
    }
}

/// Run a closure with a mutable reference to the current process.
pub fn with_current_proc<F, R>(f: F) -> Option<R>
where F: FnOnce(&mut Process) -> R {
    let pid = *CURRENT_PID.lock();
    let mut table = PROC_TABLE.lock();
    table.get_mut(&pid).map(f)
}

/// Run a closure with an immutable reference to the current process.
pub fn with_current_proc_ref<F, R>(f: F) -> Option<R>
where F: FnOnce(&Process) -> R {
    let pid = *CURRENT_PID.lock();
    let table = PROC_TABLE.lock();
    table.get(&pid).map(f)
}

pub fn run_user() -> ! {
    let (satp, sepc, sstatus, sp, ksp) = {
        let table = PROC_TABLE.lock();
        let p = table.get(&*CURRENT_PID.lock()).expect("no user process");
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
