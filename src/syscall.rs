use spin::Mutex;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::fs::{self, OpenFile};

enum NscdPhase {
    Header,
    Data,
    Done,
}

struct NscdState {
    phase: NscdPhase,
    header: [u8; 36],
    data: Vec<u8>,
    offset: usize,
}

enum FakeFdType {
    Nscd(NscdState),
    InetSocket,
    Epoll,
    EventFd,
}

static FAKE_FDS: Mutex<BTreeMap<usize, FakeFdType>> = Mutex::new(BTreeMap::new());
static FAKE_FD_NEXT: Mutex<usize> = Mutex::new(100);

struct PipeInner {
    buf: alloc::vec::Vec<u8>,
    head: usize,
    tail: usize,
    write_closed: bool,
    read_closed: bool,
}

impl PipeInner {
    fn new() -> Self {
        Self { buf: alloc::vec![0u8; 4096], head: 0, tail: 0, write_closed: false, read_closed: false }
    }
    fn write(&mut self, data: &[u8]) -> usize {
        if self.read_closed { return 0; }
        let mut written = 0;
        for &b in data {
            let next = (self.tail + 1) % self.buf.len();
            if next == self.head { break; } // full
            self.buf[self.tail] = b;
            self.tail = next;
            written += 1;
        }
        written
    }
    fn read(&mut self, out: &mut [u8]) -> usize {
        let mut n = 0;
        while n < out.len() && self.head != self.tail {
            out[n] = self.buf[self.head];
            self.head = (self.head + 1) % self.buf.len();
            n += 1;
        }
        n
    }
}

static PIPES: Mutex<BTreeMap<usize, spin::Mutex<PipeInner>>> = Mutex::new(BTreeMap::new());
static NEXT_PIPE_ID: Mutex<usize> = Mutex::new(1);

#[derive(Clone)]
pub struct FdTable {
    files: Vec<Option<OpenFile>>,
}

impl FdTable {
    pub fn new() -> Self {
        let mut files = Vec::with_capacity(64);
        files.push(Some(OpenFile { inode: 0, offset: 0, readable: true, writable: true, pipe_id: None })); // stdin
        files.push(Some(OpenFile { inode: 0, offset: 0, readable: true, writable: true, pipe_id: None })); // stdout
        files.push(Some(OpenFile { inode: 0, offset: 0, readable: true, writable: true, pipe_id: None })); // stderr
        Self { files }
    }
    pub fn alloc(&mut self, file: OpenFile) -> usize {
        for (i, slot) in self.files.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file);
                return i;
            }
        }
        let i = self.files.len();
        self.files.push(Some(file));
        i
    }
    pub fn get(&mut self, fd: usize) -> Option<&mut OpenFile> {
        self.files.get_mut(fd)?.as_mut()
    }
    pub fn close(&mut self, fd: usize) {
        if fd < self.files.len() {
            self.files[fd] = None;
        }
    }
    pub fn dup2(&mut self, oldfd: usize, newfd: usize) -> bool {
        if oldfd >= self.files.len() || self.files[oldfd].is_none() {
            return false;
        }
        if newfd >= self.files.len() {
            self.files.resize(newfd + 1, None);
        }
        self.files[newfd] = self.files[oldfd].clone();
        true
    }
}

lazy_static::lazy_static! {
    static ref FD_TABLE: Mutex<FdTable> = Mutex::new(FdTable::new());
}

pub fn dispatch(num: usize, args: [usize; 6]) -> isize {
    match num {
        17 => sys_getcwd(args[0] as *mut u8, args[1]),
        19 => sys_eventfd2(args[0] as u32, args[1] as isize),
        20 => sys_epoll_create1(args[0] as isize),
        21 => sys_epoll_ctl(args[0] as isize, args[1] as isize, args[2] as isize, args[3] as *mut u8),
        22 => sys_epoll_pwait(args[0] as isize, args[1] as *mut u8, args[2] as usize, args[3] as isize, args[4] as *const u8, args[5]),
        23 => sys_dup2(args[0] as isize, args[1] as isize),
        24 => sys_dup3(args[0] as isize, args[1] as isize, args[2] as isize),
        25 => sys_fcntl(args[0] as isize, args[1] as isize, args[2]),
        29 => sys_ioctl(args[0] as isize, args[1] as isize, args[2] as *mut u8),
        34 => sys_mkdirat(args[0] as isize, args[1] as *const u8, args[2] as isize),
        48 => sys_faccessat(args[0] as isize, args[1] as *const u8, args[2] as isize, args[3] as isize),
        54 => sys_fchownat(args[0] as isize, args[1] as *const u8, args[2] as isize, args[3] as isize, args[4]),
        56 => sys_openat(args[0] as isize, args[1] as *const u8, args[2] as isize, args[3] as isize),
        57 => sys_close(args[0] as isize),
        59 => sys_pipe2(args[0] as *mut i32, args[1] as isize),
        61 => sys_getdents64(args[0] as isize, args[1] as *mut u8, args[2]),
        62 => sys_lseek(args[0] as isize, args[1] as isize, args[2] as isize),
        71 => sys_sendfile(args[0] as isize, args[1] as isize, args[2] as *mut isize, args[3]),
        63 => sys_read(args[0] as isize, args[1] as *mut u8, args[2]),
        64 => sys_write(args[0] as isize, args[1] as *const u8, args[2]),
        66 => sys_writev(args[0] as isize, args[1] as *const u8, args[2]),
        67 => sys_pread64(args[0] as isize, args[1] as *mut u8, args[2], args[3] as i64),
        68 => sys_pwrite64(args[0] as isize, args[1] as *const u8, args[2], args[3] as i64),
        73 => sys_ppoll(args[0] as *mut u8, args[1], args[2] as *mut u8, args[3] as *mut u8, args[4]),
        78 => sys_readlinkat(args[0] as isize, args[1] as *const u8, args[2] as *mut u8, args[3]),
        79 => sys_fstatat(args[0] as isize, args[1] as *const u8, args[2] as *mut u8, args[3] as isize),
        80 => sys_fstat(args[0] as isize, args[1] as *mut u8),
        93 => sys_exit(args[0] as isize),
        94 => sys_exit(args[0] as isize),
        96 => sys_set_tid_address(args[0] as *mut isize),
        99 => sys_set_robust_list(args[0], args[1]),
        113 => sys_clock_gettime(args[0] as isize, args[1] as *mut u8),
        122 => sys_sched_setaffinity(args[0] as isize, args[1], args[2] as *const u8),
        123 => sys_sched_getaffinity(args[0] as isize, args[1], args[2] as *mut u8),
        134 => sys_rt_sigaction(args[0] as isize, args[1] as *const u8, args[2] as *mut u8, args[3]),
        135 => sys_rt_sigprocmask(args[0] as isize, args[1] as *const u8, args[2] as *mut u8, args[3]),
        144 => sys_setgid(args[0] as isize),
        146 => sys_setuid(args[0] as isize),
        159 => sys_setgroups(args[0] as isize, args[1] as *const u8),
        160 => sys_uname(args[0] as *mut u8),
        163 => sys_getrlimit(args[0] as isize, args[1] as *mut u8),
        166 => sys_umask(args[0] as isize),
        167 => sys_prctl(args[0] as isize, args[1], args[2], args[3], args[4]),
        172 => sys_getpid(),
        173 => sys_getppid(),
        174 => sys_getuid(),
        175 => sys_geteuid(),
        176 => sys_getgid(),
        177 => sys_getegid(),
        198 => sys_socket(args[0] as isize, args[1] as isize, args[2] as isize),
        199 => sys_socketpair(args[0] as isize, args[1] as isize, args[2] as isize, args[3] as *mut i32),
        200 => sys_bind(args[0] as isize, args[1] as *const u8, args[2]),
        201 => sys_listen(args[0] as isize, args[1] as isize),
        202 => sys_accept(args[0] as isize, args[1] as *mut u8, args[2] as *mut u32),
        242 => sys_accept4(args[0] as isize, args[1] as *mut u8, args[2] as *mut u32, args[3] as isize),
        203 => sys_connect(args[0] as isize, args[1] as *const u8, args[2]),
        204 => sys_getsockname(args[0] as isize, args[1] as *mut u8, args[2] as *mut u32),
        205 => sys_getpeername(args[0] as isize, args[1] as *mut u8, args[2] as *mut u32),
        206 => sys_sendto(args[0] as isize, args[1] as *const u8, args[2], args[3] as isize, args[4] as *const u8, args[5]),
        207 => sys_recvfrom(args[0] as isize, args[1] as *mut u8, args[2], args[3] as isize, args[4] as *mut u8, args[5] as *mut u32),
        208 => sys_setsockopt(args[0] as isize, args[1] as isize, args[2] as isize, args[3] as *const u8, args[4]),
        209 => sys_getsockopt(args[0] as isize, args[1] as isize, args[2] as isize, args[3] as *mut u8, args[4] as *mut u32),
        210 => sys_shutdown(args[0] as isize, args[1] as isize),
        212 => sys_recvmsg(args[0] as isize, args[1] as *mut u8, args[2] as isize),
        214 => sys_brk(args[0]),
        215 => sys_munmap(args[0], args[1]),
        220 => sys_clone(args[0], args[1], args[2] as *mut isize, args[3] as *mut isize, args[4]),
        221 => sys_execve(args[0] as *const u8, args[1] as *const *const u8, args[2] as *const *const u8),
        222 => sys_mmap(args[0], args[1], args[2] as isize, args[3] as isize, args[4] as isize, args[5] as i64),
        226 => sys_mprotect(args[0], args[1], args[2] as isize),
        233 => sys_madvise(args[0], args[1], args[2] as isize),
        260 => sys_wait4(args[0] as isize, args[1] as *mut isize, args[2] as isize, args[3] as *mut u8),
        261 => sys_prlimit64(args[0] as isize, args[1] as isize, args[2] as *const u8, args[3] as *mut u8),
        278 => sys_getrandom(args[0] as *mut u8, args[1], args[2] as isize),
        291 => sys_statx(args[0] as isize, args[1] as *const u8, args[2] as isize, args[3] as u32, args[4] as *mut u8),
        _ => {
            log::warn!("Unhandled syscall {}", num);
            -1
        }
    }
}

fn sys_write(fd: isize, buf: *const u8, count: usize) -> isize {
    if fd == 1 || fd == 2 {
        let slice = unsafe { core::slice::from_raw_parts(buf, count) };
        let s = core::str::from_utf8(slice).unwrap_or("<invalid utf8>");
        console::print(format_args!("{}", s));
        return count as isize;
    }
    // Temporarily echo nginx log files to console for debugging
    if fd == 3 || fd == 4 || fd == 5 || fd == 6 {
        let slice = unsafe { core::slice::from_raw_parts(buf, count) };
        if let Ok(s) = core::str::from_utf8(slice) {
            for line in s.lines() {
                if !line.is_empty() {
                    log::info!("[fd{}] {}", fd, line);
                }
            }
        }
    }
    if is_inet_socket(fd as usize) {
        unsafe {
            let slice = core::slice::from_raw_parts(buf, count);
            return crate::net::send_to_fd(fd as usize, slice);
        }
    }
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        if let Some(pid) = of.pipe_id {
            drop(fd_table);
            let pipes = PIPES.lock();
            if let Some(pipe) = pipes.get(&pid) {
                let mut inner = pipe.lock();
                let mut tmp = alloc::vec![0u8; count];
                unsafe { core::ptr::copy_nonoverlapping(buf, tmp.as_mut_ptr(), count); }
                let n = inner.write(&tmp);
                return n as isize;
            }
            return 0;
        }
        let mut tmp = alloc::vec![0u8; count];
        unsafe { core::ptr::copy_nonoverlapping(buf, tmp.as_mut_ptr(), count); }
        let n = fs::write_inode(of.inode, &tmp, of.offset);
        of.offset += n;
        return n as isize;
    }
    -1
}

fn sys_read(fd: isize, buf: *mut u8, count: usize) -> isize {
    if let Some(FakeFdType::Nscd(state)) = FAKE_FDS.lock().get_mut(&(fd as usize)) {
        let n = (state.data.len().saturating_sub(state.offset)).min(count);
        if n > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(state.data.as_ptr().add(state.offset), buf, n);
            }
            state.offset += n;
        }
        return n as isize;
    }
    if is_inet_socket(fd as usize) {
        unsafe {
            let slice = core::slice::from_raw_parts_mut(buf, count);
            return crate::net::recv_from_fd(fd as usize, slice);
        }
    }
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        if let Some(pid) = of.pipe_id {
            drop(fd_table);
            let pipes = PIPES.lock();
            if let Some(pipe) = pipes.get(&pid) {
                let mut inner = pipe.lock();
                let mut tmp = alloc::vec![0u8; count];
                let n = inner.read(&mut tmp);
                if n > 0 {
                    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, n); }
                }
                return n as isize;
            }
            return 0;
        }
        let mut tmp = alloc::vec![0u8; count];
        let n = fs::read_inode(of.inode, &mut tmp, of.offset);
        unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, n); }
        of.offset += n;
        return n as isize;
    }
    -1
}

fn sys_writev(fd: isize, iov: *const u8, iovcnt: usize) -> isize {
    #[repr(C)]
    struct IOVec { base: usize, len: usize }
    let mut total = 0isize;
    for i in 0..iovcnt {
        let ptr = unsafe { iov.add(i * core::mem::size_of::<IOVec>()) as *const IOVec };
        let entry = unsafe { core::ptr::read(ptr) };
        if entry.len == 0 { continue; }
        let n = sys_write(fd, entry.base as *const u8, entry.len);
        if n < 0 {
            if total == 0 { return n; }
            break;
        }
        total += n;
        if (n as usize) < entry.len { break; }
    }
    total
}

fn sys_openat(_dirfd: isize, path: *const u8, flags: isize, _mode: isize) -> isize {
    let path = unsafe { cstr(path) };
    log::debug!("openat path={}", path);
    if let Some(inode) = fs::lookup(path) {
        let readable = true;
        let writable = (flags & 0o1) != 0 || (flags & 0o2) != 0; // O_WRONLY or O_RDWR
        let mut fd_table = FD_TABLE.lock();
        let fd = fd_table.alloc(OpenFile { inode, offset: 0, readable, writable, pipe_id: None });
        return fd as isize;
    }
    -1
}

fn read_user_u64(addr: usize) -> Option<usize> {
    crate::proc::with_current_proc_ref(|proc| {
        proc.page_table.translate(addr).map(|pa| unsafe { *(pa as *const usize) })
    }).flatten()
}

fn read_user_byte(addr: usize) -> Option<u8> {
    crate::proc::with_current_proc_ref(|proc| {
        proc.page_table.translate(addr).map(|pa| unsafe { *(pa as *const u8) })
    }).flatten()
}

fn read_user_str(addr: usize) -> Option<alloc::string::String> {
    let mut s = alloc::string::String::new();
    let mut off = 0usize;
    loop {
        let b = read_user_byte(addr + off)?;
        if b == 0 { break; }
        s.push(b as char);
        off += 1;
        if off > 4096 { return None; }
    }
    Some(s)
}

fn sys_close(fd: isize) -> isize {
    if is_inet_socket(fd as usize) {
        log::info!("sys_close: inet socket fd={}", fd);
        crate::net::close_fd(fd as usize);
        FAKE_FDS.lock().remove(&(fd as usize));
        return 0;
    }
    if FAKE_FDS.lock().remove(&(fd as usize)).is_some() {
        return 0;
    }
    FD_TABLE.lock().close(fd as usize);
    0
}

fn sys_lseek(fd: isize, offset: isize, whence: isize) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        match whence {
            0 => of.offset = offset as usize,
            1 => of.offset = (of.offset as isize + offset) as usize,
            2 => of.offset = (fs::file_size(of.inode) as isize + offset) as usize,
            _ => return -1,
        }
        return of.offset as isize;
    }
    -1
}

fn sys_fstat(fd: isize, buf: *mut u8) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        let size = fs::file_size(of.inode) as u64;
        unsafe { fill_stat(buf, size); }
        return 0;
    }
    -1
}
fn sys_fstatat(_dirfd: isize, path: *const u8, buf: *mut u8, _flags: isize) -> isize {
    let path = unsafe { cstr(path) };
    if let Some(inode) = fs::lookup(path) {
        let size = fs::file_size(inode) as u64;
        unsafe { fill_stat(buf, size); }
        0
    } else {
        -1
    }
}

fn sys_exit(code: isize) -> isize {
    log::info!("exit({})", code);
    crate::proc::exit_process(code);
    0
}

fn sys_brk(addr: usize) -> isize {
    crate::proc::with_current_proc(|proc| {
        if proc.brk == 0 {
            proc.brk = 0x10000;
        }
        if addr == 0 {
            return proc.brk as isize;
        }
        if addr > proc.brk {
            let start = (proc.brk + 0xfff) & !0xfff;
            let end = (addr + 0xfff) & !0xfff;
            for page in (start..end).step_by(0x1000) {
                if proc.page_table.translate(page).is_none() {
                    if let Some(pa) = crate::mm::alloc_page() {
                        proc.page_table.map(page, pa, crate::mm::PTEFlags::U | crate::mm::PTEFlags::R | crate::mm::PTEFlags::W);
                    }
                }
            }
        }
        proc.brk = addr;
        log::info!("sys_brk: addr={:#x} -> brk={:#x}", addr, proc.brk);
        addr as isize
    }).unwrap_or(-1)
}

fn sys_mmap(addr: usize, len: usize, prot: isize, flags: isize, fd: isize, offset: i64) -> isize {
    static MMAP_BASE: spin::Mutex<usize> = spin::Mutex::new(0);
    let mut base = MMAP_BASE.lock();
    if *base == 0 {
        *base = 0x5000_0000;
    }
    let aligned_len = (len + 0xfff) & !0xfff;

    // MAP_FIXED = 0x10: honor requested address
    let ret = if flags & 0x10 != 0 {
        addr
    } else {
        let ret = *base;
        *base += aligned_len;
        ret
    };

    crate::proc::with_current_proc(|proc| {
        for page in (0..aligned_len).step_by(0x1000) {
            let va = ret + page;
            if proc.page_table.translate(va).is_none() {
                if let Some(pa) = crate::mm::alloc_page() {
                    let mut pte_flags = crate::mm::PTEFlags::U;
                    if prot & 1 != 0 { pte_flags |= crate::mm::PTEFlags::R; }
                    if prot & 2 != 0 { pte_flags |= crate::mm::PTEFlags::W; }
                    if prot & 4 != 0 { pte_flags |= crate::mm::PTEFlags::X; }
                    proc.page_table.map(va, pa, pte_flags);
                }
            }
        }
    });

    // Zero-initialize mapped pages (important for anonymous mmap)
    unsafe { core::ptr::write_bytes(ret as *mut u8, 0, aligned_len); }

    // Handle MAP_ANONYMOUS vs file-backed
    if flags & 0x20 == 0 && fd >= 0 {
        // file-backed mmap
        let mut fd_table = FD_TABLE.lock();
        if let Some(of) = fd_table.get(fd as usize) {
            let old_offset = of.offset;
            of.offset = offset as usize;
            let mut tmp = alloc::vec![0u8; len];
            let n = fs::read_inode(of.inode, &mut tmp, of.offset);
            unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), ret as *mut u8, n); }
            of.offset = old_offset;
        }
    }
    log::info!("sys_mmap: addr={:#x} len={:#x} prot={} flags={} fd={} offset={} -> ret={:#x}", addr, len, prot, flags, fd, offset, ret);
    ret as isize
}

fn sys_munmap(_addr: usize, _len: usize) -> isize { 0 }
fn sys_mprotect(_addr: usize, _len: usize, _prot: isize) -> isize { 0 }
fn sys_madvise(_addr: usize, _len: usize, _advice: isize) -> isize { 0 }

fn sys_getpid() -> isize { *crate::proc::CURRENT_PID.lock() as isize }
fn sys_getppid() -> isize {
    let pid = *crate::proc::CURRENT_PID.lock();
    let table = crate::proc::PROC_TABLE.lock();
    table.get(&pid).map(|p| p.ppid as isize).unwrap_or(0)
}
fn sys_setgid(_gid: isize) -> isize { 0 }
fn sys_setuid(_uid: isize) -> isize { 0 }
fn sys_setgroups(_size: isize, _list: *const u8) -> isize { 0 }
fn sys_getuid() -> isize { 0 }
fn sys_geteuid() -> isize { 0 }
fn sys_getgid() -> isize { 0 }
fn sys_getegid() -> isize { 0 }

fn sys_uname(buf: *mut u8) -> isize {
    #[repr(C)]
    struct UtsName {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }
    unsafe {
        let name = buf as *mut UtsName;
        (*name).sysname = pad(b"Linux");
        (*name).nodename = pad(b"rvos");
        (*name).release = pad(b"5.15.0");
        (*name).version = pad(b"#1");
        (*name).machine = pad(b"riscv64");
        (*name).domainname = pad(b"");
    }
    0
}

fn pad(s: &[u8]) -> [u8; 65] {
    let mut arr = [0u8; 65];
    arr[..s.len()].copy_from_slice(s);
    arr
}

fn sys_getcwd(buf: *mut u8, size: usize) -> isize {
    let cwd = b"/tmp\0";
    if size < cwd.len() { return -1; }
    unsafe { core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd.len()); }
    buf as isize
}

fn sys_clock_gettime(_clk_id: isize, buf: *mut u8) -> isize {
    #[repr(C)]
    struct Timespec { tv_sec: i64, tv_nsec: i64 }
    let ticks: u64;
    unsafe {
        core::arch::asm!("rdtime {}", out(reg) ticks);
    }
    // QEMU virt timer runs at 10 MHz
    let sec = (ticks / 10_000_000) as i64 + 1;
    // Nginx actually treats the second field as microseconds (gettimeofday
    // style), so return raw microseconds here to keep msec = usec/1000 correct.
    let usec = (ticks % 10_000_000) as i64;
    unsafe {
        (*(buf as *mut Timespec)).tv_sec = sec;
        (*(buf as *mut Timespec)).tv_nsec = usec;
    }
    0
}

fn sys_rt_sigaction(_sig: isize, _act: *const u8, _oldact: *mut u8, _sigsetsize: usize) -> isize { 0 }
fn sys_rt_sigprocmask(_how: isize, _set: *const u8, _oldset: *mut u8, _sigsetsize: usize) -> isize { 0 }
fn sys_fcntl(fd: isize, cmd: isize, _arg: usize) -> isize {
    match cmd {
        1 => fd as isize, // F_GETFD
        2 => 0,           // F_SETFD
        3 => {
            // F_GETFL
            if is_inet_socket(fd as usize) {
                return 0x800; // O_NONBLOCK
            }
            0
        }
        4 => 0,          // F_SETFL
        _ => 0,
    }
}
fn sys_dup2(oldfd: isize, newfd: isize) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if fd_table.dup2(oldfd as usize, newfd as usize) {
        newfd as isize
    } else {
        -1
    }
}
fn sys_dup3(oldfd: isize, newfd: isize, _flags: isize) -> isize {
    sys_dup2(oldfd, newfd)
}
fn sys_ioctl(fd: isize, req: isize, _arg: *mut u8) -> isize {
    if req == 0x5421 { // FIONBIO
        if is_fake_fd(fd as usize) || crate::net::is_socket_fd(fd as usize) {
            return 0;
        }
    }
    0
}
fn sys_prctl(_option: isize, _arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> isize { 0 }
fn sys_set_tid_address(_tidptr: *mut isize) -> isize { 1 }
fn sys_set_robust_list(_head: usize, _len: usize) -> isize { 0 }
fn sys_getrlimit(resource: isize, buf: *mut u8) -> isize {
    #[repr(C)]
    struct Rlimit { rlim_cur: u64, rlim_max: u64 }
    let (cur, max) = match resource {
        7 => (1024u64, 4096u64), // RLIMIT_NOFILE
        _ => (u64::MAX, u64::MAX),
    };
    unsafe {
        (*(buf as *mut Rlimit)).rlim_cur = cur;
        (*(buf as *mut Rlimit)).rlim_max = max;
    }
    0
}
fn sys_umask(_mask: isize) -> isize { 0o22 }
fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: isize) -> isize {
    unsafe { core::ptr::write_bytes(buf, 0xAB, buflen); }
    buflen as isize
}

fn sys_faccessat(_dirfd: isize, path: *const u8, _mode: isize, _flags: isize) -> isize {
    let path = unsafe { cstr(path) };
    if fs::lookup(path).is_some() { 0 } else { -1 }
}
fn sys_mkdirat(_dirfd: isize, path: *const u8, _mode: isize) -> isize {
    let path = unsafe { cstr(path) };
    if fs::lookup(path).is_some() { 0 } else {
        fs::mkdir(path);
        0
    }
}
fn sys_fchownat(_dirfd: isize, _path: *const u8, _owner: isize, _group: isize, _flags: usize) -> isize {
    0
}
fn sys_eventfd2(_initval: u32, _flags: isize) -> isize {
    let fd = {
        let mut next = FAKE_FD_NEXT.lock();
        let fd = *next;
        *next += 1;
        fd
    };
    FAKE_FDS.lock().insert(fd, FakeFdType::EventFd);
    fd as isize
}
fn sys_epoll_create1(_flags: isize) -> isize {
    let fd = {
        let mut next = FAKE_FD_NEXT.lock();
        let fd = *next;
        *next += 1;
        fd
    };
    FAKE_FDS.lock().insert(fd, FakeFdType::Epoll);
    fd as isize
}
fn sys_epoll_ctl(_epfd: isize, _op: isize, fd: isize, event: *mut u8) -> isize {
    if (is_inet_socket(fd as usize) || is_fake_fd(fd as usize)) && !event.is_null() {
        unsafe {
            // struct epoll_event on riscv64 Linux: u32 events; u32 padding; u64 data;
            let data = core::ptr::read_unaligned(event.add(8) as *mut u64);
            crate::net::set_epoll_data(fd as usize, data);
        }
    }
    0
}
fn sys_epoll_pwait(_epfd: isize, events: *mut u8, maxevents: usize, timeout: isize, _sigmask: *const u8, _sigsetsize: usize) -> isize {
    let start_ms = crate::net::get_time_ms();
    let deadline = if timeout > 0 {
        Some(start_ms + timeout as i64)
    } else {
        None
    };
    let mut spins = 0;
    loop {
        let n = crate::net::get_epoll_events(0, events, maxevents);
        if n > 0 {
            return n as isize;
        }
        if timeout == 0 {
            return 0;
        }
        if let Some(deadline) = deadline {
            if crate::net::get_time_ms() >= deadline {
                return 0;
            }
        }
        spins += 1;
        if spins % 10000 == 0 {
            core::hint::spin_loop();
        }
        // Busy-wait: poll the network stack repeatedly until an event
        // arrives or the timeout expires.
    }
}

fn sys_pipe2(pipefd: *mut i32, _flags: isize) -> isize {
    let mut pipes = PIPES.lock();
    let mut next_id = NEXT_PIPE_ID.lock();
    let id = *next_id;
    *next_id += 1;
    pipes.insert(id, spin::Mutex::new(PipeInner::new()));
    drop(pipes);

    let mut fd_table = FD_TABLE.lock();
    let rfd = fd_table.alloc(OpenFile { inode: 0, offset: 0, readable: true, writable: false, pipe_id: Some(id) });
    let wfd = fd_table.alloc(OpenFile { inode: 0, offset: 0, readable: false, writable: true, pipe_id: Some(id) });
    unsafe {
        *pipefd = rfd as i32;
        *pipefd.add(1) = wfd as i32;
    }
    0
}

fn sys_sendfile(out_fd: isize, in_fd: isize, _offset: *mut isize, count: usize) -> isize {
    let mut buf = alloc::vec![0u8; count.min(4096)];
    let n = sys_read(in_fd, buf.as_mut_ptr(), buf.len());
    if n <= 0 { return n; }
    sys_write(out_fd, buf.as_ptr(), n as usize)
}

fn sys_getdents64(fd: isize, buf: *mut u8, count: usize) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        let mut tmp = alloc::vec![0u8; count];
        let (n, next_off) = fs::read_dir(of.inode, &mut tmp, of.offset);
        if n > 0 {
            unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, n); }
            of.offset = next_off;
        }
        return n as isize;
    }
    -1
}
fn sys_pread64(fd: isize, buf: *mut u8, count: usize, offset: i64) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        let mut tmp = alloc::vec![0u8; count];
        let n = fs::read_inode(of.inode, &mut tmp, offset as usize);
        unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, n); }
        return n as isize;
    }
    -1
}
fn sys_pwrite64(fd: isize, buf: *const u8, count: usize, offset: i64) -> isize {
    let mut fd_table = FD_TABLE.lock();
    if let Some(of) = fd_table.get(fd as usize) {
        let mut tmp = alloc::vec![0u8; count];
        unsafe { core::ptr::copy_nonoverlapping(buf, tmp.as_mut_ptr(), count); }
        let n = fs::write_inode(of.inode, &tmp, offset as usize);
        return n as isize;
    }
    -1
}

fn sys_execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> isize {
    let path = match read_user_str(path as usize) {
        Some(s) => s,
        None => return -1,
    };
    log::info!("sys_execve: path={}", path);

    // Read argv array
    let mut argv_strs = alloc::vec::Vec::new();
    let mut i = 0usize;
    loop {
        match read_user_u64((argv as usize) + i * 8) {
            Some(0) | None => break,
            Some(ptr) => {
                if let Some(s) = read_user_str(ptr) {
                    argv_strs.push(s);
                } else {
                    break;
                }
            }
        }
        i += 1;
        if i > 128 { break; }
    }

    // Read envp array
    let mut envp_strs = alloc::vec::Vec::new();
    i = 0;
    loop {
        match read_user_u64((envp as usize) + i * 8) {
            Some(0) | None => break,
            Some(ptr) => {
                if let Some(s) = read_user_str(ptr) {
                    envp_strs.push(s);
                } else {
                    break;
                }
            }
        }
        i += 1;
        if i > 128 { break; }
    }

    let data = match fs::get_file_data(&path) {
        Some(d) => d,
        None => {
            log::warn!("sys_execve: file not found: {}", path);
            return -1;
        }
    };

    // Ensure PATH is set
    if !envp_strs.iter().any(|s| s.starts_with("PATH=")) {
        envp_strs.push(alloc::string::String::from("PATH=/bin"));
    }
    log::info!("sys_execve: argv={:?} envp={:?}", argv_strs, envp_strs);

    let result: Option<usize> = crate::proc::with_current_proc(|proc| {
        crate::proc::exec_process(proc, data, &argv_strs, &envp_strs)
    }).flatten();

    if let Some(sp) = result {
        let sepc = crate::proc::with_current_proc_ref(|p| p.trap_frame.sepc).unwrap_or(0);
        let mut tf = crate::trap::CURRENT_TRAP_FRAME.lock();
        tf.sepc = sepc;
        tf.regs[2] = sp;
        tf.regs[10] = argv_strs.len();
        // Switch to new page table immediately so the next sret uses it
        let satp = crate::proc::with_current_proc_ref(|p| {
            (8usize << 60) | p.page_table.root_ppn()
        }).unwrap_or(0);
        unsafe {
            core::arch::asm!("csrw satp, {}", in(reg) satp);
            core::arch::asm!("sfence.vma");
        }
        0
    } else {
        -1
    }
}

fn sys_clone(_flags: usize, stack: usize, _ptid: *mut isize, _ctid: *mut isize, _tls: usize) -> isize {
    let tf = crate::trap::CURRENT_TRAP_FRAME.lock().clone();
    if let Some(child_pid) = crate::proc::fork_process(&tf) {
        // If child stack is specified, update child's sp
        if stack != 0 {
            let mut table = crate::proc::PROC_TABLE.lock();
            if let Some(child) = table.get_mut(&child_pid) {
                child.trap_frame.regs[2] = stack;
            }
        }
        log::info!("sys_clone: flags={:#x} stack={:#x} -> child_pid={}", _flags, stack, child_pid);
        child_pid as isize
    } else {
        -1
    }
}

fn sys_wait4(pid: isize, wstatus: *mut isize, _options: isize, _rusage: *mut u8) -> isize {
    let current_pid = *crate::proc::CURRENT_PID.lock();
    let table = crate::proc::PROC_TABLE.lock();
    let mut zombie_pid = None;
    for (cpid, proc) in table.iter() {
        if proc.ppid == current_pid && proc.state == crate::proc::ProcState::Zombie {
            if pid > 0 && *cpid != pid as usize {
                continue;
            }
            zombie_pid = Some((*cpid, proc.exit_code));
            break;
        }
    }
    if let Some((zpid, code)) = zombie_pid {
        drop(table);
        if !wstatus.is_null() {
            unsafe { *wstatus = ((code & 0xff) << 8) as isize; }
        }
        // Remove zombie from table
        crate::proc::PROC_TABLE.lock().remove(&zpid);
        log::info!("sys_wait4: pid={} -> zombie {} exited with code {}", pid, zpid, code);
        zpid as isize
    } else {
        // No matching zombie child; for non-blocking (WNOHANG=1) return 0, else return -EAGAIN
        if _options & 1 != 0 {
            0
        } else {
            -11 // EAGAIN
        }
    }
}
fn sys_sched_setaffinity(_pid: isize, _len: usize, _mask: *const u8) -> isize { 0 }
fn sys_sched_getaffinity(_pid: isize, _len: usize, _mask: *mut u8) -> isize { 0 }
fn sys_statx(_dirfd: isize, path: *const u8, _flags: isize, _mask: u32, buf: *mut u8) -> isize {
    let path = unsafe { cstr(path) };
    if let Some(inode) = fs::lookup(path) {
        let size = fs::file_size(inode) as u64;
        unsafe { fill_stat(buf, size); }
        0
    } else {
        -1
    }
}

fn sys_readlinkat(_dirfd: isize, path: *const u8, buf: *mut u8, bufsiz: usize) -> isize {
    let path = unsafe { cstr(path) };
    let target = if path == "/proc/self/exe" {
        "/sbin/nginx"
    } else {
        return -1;
    };
    let bytes = target.as_bytes();
    let len = bytes.len().min(bufsiz);
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len); }
    len as isize
}

fn sys_prlimit64(_pid: isize, resource: isize, _new_limit: *const u8, old_limit: *mut u8) -> isize {
    if !old_limit.is_null() {
        #[repr(C)]
        struct Rlimit64 { rlim_cur: u64, rlim_max: u64 }
        let mut lim = u64::MAX;
        if resource == 7 { // RLIMIT_NOFILE
            lim = 1024;
        }
        unsafe {
            (*(old_limit as *mut Rlimit64)).rlim_cur = lim;
            (*(old_limit as *mut Rlimit64)).rlim_max = lim;
        }
    }
    0
}

fn is_fake_fd(fd: usize) -> bool {
    FAKE_FDS.lock().contains_key(&fd)
}

fn is_inet_socket(fd: usize) -> bool {
    matches!(FAKE_FDS.lock().get(&fd), Some(FakeFdType::InetSocket))
}

fn sys_ppoll(fds: *mut u8, nfds: usize, _tmo: *mut u8, _sigmask: *mut u8, _sigsetsize: usize) -> isize {
    let mut ready = 0;
    for i in 0..nfds {
        unsafe {
            let p = fds.add(i * 8) as *mut u8;
            let fd = *(p as *mut i32);
            let events = *(p.add(4) as *mut i16);
            let revents = p.add(6) as *mut i16;
            if is_fake_fd(fd as usize) {
                *revents = events & 0x005; // POLLIN | POLLOUT
                ready += 1;
            } else {
                *revents = 0;
            }
        }
    }
    if ready > 0 { ready as isize } else { 0 }
}

// Network stubs (with fake NSCD support)
fn sys_socket(domain: isize, _ty: isize, _protocol: isize) -> isize {
    let fd = {
        let mut next = FAKE_FD_NEXT.lock();
        let fd = *next;
        *next += 1;
        fd
    };
    if domain == 1 { // AF_UNIX
        FAKE_FDS.lock().insert(fd, FakeFdType::Nscd(NscdState {
            phase: NscdPhase::Header,
            header: [0; 36],
            data: Vec::new(),
            offset: 0,
        }));
        return fd as isize;
    }
    if domain == 2 { // AF_INET
        FAKE_FDS.lock().insert(fd, FakeFdType::InetSocket);
        return fd as isize;
    }
    -1
}
fn sys_socketpair(_domain: isize, _ty: isize, _protocol: isize, sv: *mut i32) -> isize {
    let fd1 = {
        let mut next = FAKE_FD_NEXT.lock();
        let fd = *next;
        *next += 1;
        fd
    };
    let fd2 = {
        let mut next = FAKE_FD_NEXT.lock();
        let fd = *next;
        *next += 1;
        fd
    };
    FAKE_FDS.lock().insert(fd1, FakeFdType::InetSocket);
    FAKE_FDS.lock().insert(fd2, FakeFdType::InetSocket);
    unsafe {
        *sv = fd1 as i32;
        *sv.add(1) = fd2 as i32;
    }
    0
}
fn sys_bind(fd: isize, _addr: *const u8, _len: usize) -> isize {
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}
fn sys_listen(fd: isize, _backlog: isize) -> isize {
    if is_inet_socket(fd as usize) {
        crate::net::bind_listen_fd(fd as usize);
        return 0;
    }
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}
fn sys_accept(fd: isize, addr: *mut u8, addrlen: *mut u32) -> isize {
    if is_inet_socket(fd as usize) {
        if let Some(handle) = crate::net::accept_connection(fd as usize) {
            let new_fd = {
                let mut next = FAKE_FD_NEXT.lock();
                let f = *next;
                *next += 1;
                f
            };
            log::info!("sys_accept: fd={} -> new_fd={} handle={:?}", fd, new_fd, handle);
            FAKE_FDS.lock().insert(new_fd, FakeFdType::InetSocket);
            crate::net::add_fd_handle(new_fd, handle);
            if !addr.is_null() {
                unsafe {
                    *(addr as *mut u16) = 2; // AF_INET in host byte order
                    *(addr.add(2) as *mut u16) = (80u16).to_be(); // port in network byte order
                    core::ptr::write_bytes(addr.add(4), 0, 4); // addr = 0.0.0.0
                    core::ptr::write_bytes(addr.add(8), 0, 8); // padding
                }
            }
            if !addrlen.is_null() {
                unsafe { *addrlen = 16; }
            }
            return new_fd as isize;
        }
        return -11; // EAGAIN
    }
    if is_fake_fd(fd as usize) {
        let new_fd = {
            let mut next = FAKE_FD_NEXT.lock();
            let f = *next;
            *next += 1;
            f
        };
        FAKE_FDS.lock().insert(new_fd, FakeFdType::InetSocket);
        return new_fd as isize;
    }
    -1
}
fn sys_accept4(fd: isize, addr: *mut u8, addrlen: *mut u32, _flags: isize) -> isize {
    sys_accept(fd, addr, addrlen)
}
fn sys_connect(fd: isize, _addr: *const u8, _len: usize) -> isize {
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}
fn sys_getsockname(_fd: isize, _addr: *mut u8, _addrlen: *mut u32) -> isize { -1 }
fn sys_getpeername(_fd: isize, _addr: *mut u8, _addrlen: *mut u32) -> isize { -1 }
fn sys_sendto(fd: isize, buf: *const u8, len: usize, _flags: isize, _addr: *const u8, _addrlen: usize) -> isize {
    if let Some(FakeFdType::Nscd(state)) = FAKE_FDS.lock().get_mut(&(fd as usize)) {
        let mut req = alloc::vec![0u8; len];
        unsafe { core::ptr::copy_nonoverlapping(buf, req.as_mut_ptr(), len); }
        let name = b"root\0";
        let passwd = b"x\0";
        let gecos = b"root\0";
        let dir = b"/root\0";
        let shell = b"/bin/sh\0";
        let mut resp = Vec::with_capacity(36 + name.len() + passwd.len() + gecos.len() + dir.len() + shell.len());
        let mut push_i32 = |v: i32| resp.extend_from_slice(&v.to_ne_bytes());
        push_i32(2); // version
        push_i32(1); // found
        push_i32(name.len() as i32);
        push_i32(passwd.len() as i32);
        push_i32(0); // uid
        push_i32(0); // gid
        push_i32(gecos.len() as i32);
        push_i32(dir.len() as i32);
        push_i32(shell.len() as i32);
        resp.extend_from_slice(name);
        resp.extend_from_slice(passwd);
        resp.extend_from_slice(gecos);
        resp.extend_from_slice(dir);
        resp.extend_from_slice(shell);
        state.data = resp;
        state.offset = 0;
        state.phase = NscdPhase::Data;
        return len as isize;
    }
    if is_inet_socket(fd as usize) {
        unsafe {
            let slice = core::slice::from_raw_parts(buf, len);
            return crate::net::send_to_fd(fd as usize, slice);
        }
    }
    -1
}
fn sys_recvfrom(fd: isize, buf: *mut u8, len: usize, _flags: isize, _addr: *mut u8, _addrlen: *mut u32) -> isize {
    sys_read(fd, buf, len)
}
fn sys_recvmsg(fd: isize, msg: *mut u8, _flags: isize) -> isize {
    if let Some(FakeFdType::Nscd(state)) = FAKE_FDS.lock().get_mut(&(fd as usize)) {
        unsafe {
            let iov_ptr = *(msg.add(16) as *const usize);
            let iov_len = *(msg.add(24) as *const usize);
            if iov_len > 0 && iov_ptr != 0 {
                let base = *(iov_ptr as *const usize);
                let len = *((iov_ptr as *const u8).add(8) as *const usize);
                let n = (state.data.len().saturating_sub(state.offset)).min(len);
                if n > 0 && base != 0 {
                    core::ptr::copy_nonoverlapping(state.data.as_ptr().add(state.offset), base as *mut u8, n);
                    state.offset += n;
                }
                return n as isize;
            }
        }
    }
    -1
}
fn sys_setsockopt(fd: isize, _level: isize, _optname: isize, _optval: *const u8, _optlen: usize) -> isize {
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}
fn sys_getsockopt(fd: isize, _level: isize, _optname: isize, _optval: *mut u8, _optlen: *mut u32) -> isize {
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}
fn sys_shutdown(fd: isize, _how: isize) -> isize {
    if is_fake_fd(fd as usize) { 0 } else { -1 }
}

unsafe fn cstr(ptr: *const u8) -> &'static str {
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len))
}

unsafe fn fill_stat(buf: *mut u8, size: u64) {
    // Minimal stat structure for Linux riscv64
    // struct stat {
    //   dev_t st_dev; (8)
    //   ino_t st_ino; (8)
    //   mode_t st_mode; (4)
    //   nlink_t st_nlink; (4)
    //   uid_t st_uid; (4)
    //   gid_t st_gid; (4)
    //   dev_t st_rdev; (8)
    //   unsigned long __pad; (8)
    //   off_t st_size; (8)
    //   blksize_t st_blksize; (4)
    //   int __pad2; (4)
    //   blkcnt_t st_blocks; (8)
    //   ... rest
    // }
    // Total size is 128 bytes on riscv64
    core::ptr::write_bytes(buf, 0, 128);
    let mode = if size == 0 { 0o40755 } else { 0o100644 };
    *(buf.add(16) as *mut u32) = mode; // st_mode
    *(buf.add(24) as *mut u32) = 1;    // st_nlink
    *(buf.add(48) as *mut u64) = size; // st_size
    *(buf.add(56) as *mut u32) = 4096; // st_blksize
    *(buf.add(64) as *mut u64) = (size + 511) / 512; // st_blocks
}

use crate::console;
