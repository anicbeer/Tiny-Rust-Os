# RVOS — RISC-V Operating System in Rust

A minimal RISC-V 64-bit OS kernel written in Rust (`#![no_std]`), capable of running unmodified statically-linked Linux binaries (busybox shell and nginx) under QEMU with virtio-net networking.

## Features

- **RISC-V 64** (`rv64gc`) bare-metal kernel, SV39 virtual memory
- **VirtIO-MMIO** virtio-net device driver
- **smoltcp 0.11** TCP/IP stack with full socket syscall emulation (`socket`, `bind`, `listen`, `accept`, `sendto`, `recvfrom`)
- **Linux syscall compatibility layer** — emulates ~50 syscalls so unmodified glibc static binaries can run without recompilation
- **Multi-process support** — `clone`/`fork`, `execve`, `wait4`, `exit`, and round-robin cooperative scheduling with blocking wait
- **Shell & pipes** — busybox `sh` with `fork`+`execve`, pipe2 (`cmd1 | cmd2`), and batch-mode command reading
- **In-memory RAMFS** with static and dynamic file support and `getdents64` directory listing
- **ELF loading** with full `PT_LOAD` segment mapping, auxv initialization, and per-process `brk`
- **User/kernel mode switching** via S-mode trap handler with complete register save/restore

## Architecture

```
+-------------------------------------------+
|  User Space: busybox sh, nginx, cat, ls   |
+-------------------------------------------+
|  Linux Syscall Emulation (src/syscall.rs) |
+-------------------------------------------+
|  Process & ELF  |  Network Stack (smoltcp)|
|  (src/proc.rs)  |  (src/net.rs)           |
+-------------------------------------------+
|  Trap Handler   |  VirtIO-Net Driver      |
|  (src/trap.rs)  |  (src/virtio.rs)        |
+-------------------------------------------+
|  Memory Management (SV39 page tables)     |
|  (src/mm.rs)                                |
+-------------------------------------------+
|  UART Console |  SBI |  RAMFS (src/fs.rs) |
+-------------------------------------------+
```

### Module Overview

| File | Responsibility |
|------|----------------|
| `entry.asm` | Boot entry: BSS zeroing, stack setup, call `rust_main` |
| `linker.ld` | Kernel linked at `0x8020_0000` |
| `main.rs` | Init sequence: console → mm → heap → page table → trap → virtio → net → fs → proc → run busybox shell |
| `console.rs` | UART output (`print`) and input (`getchar`) via 16550 registers |
| `mm.rs` | Bump heap allocator, page frame allocator, SV39 `PageTable` with `map`/`translate` |
| `trap.rs` | Naked trap vector in assembly: swap to kernel stack, save all registers, set `sstatus.SUM`, call `rust_trap_handler`, restore and `sret` |
| `virtio.rs` | VirtIO-MMIO transport + `VirtIONet` driver via `virtio-drivers` crate |
| `net.rs` | `smoltcp` `Device` impl bridging virtio-net RX/TX; socket fd management, TCP listen/accept/send/recv |
| `fs.rs` | In-memory BTreeMap-based filesystem; `FileContents::Static` for embedded blobs, `FileContents::Dynamic` for writable logs |
| `proc.rs` | `Process` struct with SV39 page table, `TrapFrame`, kernel stack, per-process `brk`; ELF loader with auxv setup |
| `syscall.rs` | Syscall dispatcher for ~50 Linux syscalls; fd tables, pipes, fake fds for epoll/eventfd/NSCD, socket emulation |

## Build

Requires Rust nightly with `riscv64gc-unknown-none-elf` target:

```bash
rustup target add riscv64gc-unknown-none-elf
cargo build --release
```

**Note**: The kernel embeds pre-built static binaries at compile time via `include_bytes!`:
- `/tmp/busybox-1.36.1/busybox` → `/bin/sh`, `/bin/ls`, `/bin/cat`, etc.
- `/tmp/nginx-1.26.3/objs/nginx` → `/sbin/nginx`
- `../libnss_files.so.2` → `/lib/riscv64-linux-gnu/libnss_files.so.2`

If you modify these binaries, you must rebuild the kernel for changes to take effect.

## Run

### Default: busybox shell

The kernel boots into a busybox `sh` that reads commands from stdin (batch mode, since `isatty()` returns false).

```bash
qemu-system-riscv64 \
  -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

Pipe commands to it:

```bash
(echo "ls /"; echo "exit") | \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

Expected output:

```
bin       lib       sbin      test_malloc  usr
etc       proc      sys       tmp          var
```

If you send commands too early, the first character may be lost due to UART init timing. Add a 2–3 second delay:

```bash
(sleep 3; echo "ls /"; sleep 2; echo "exit") | \
qemu-system-riscv64 ...
```

### Run nginx instead

Edit `src/main.rs` and replace the shell startup with nginx:

```rust
// let sh_data = fs::get_file_data("/bin/sh").expect("sh binary not found");
// proc::init_user_proc(sh_data, &["sh"]).expect("failed to load shell");

let nginx_data = fs::get_file_data("/bin/nginx").expect("nginx binary not found");
proc::init_user_proc(nginx_data, &["nginx"]).expect("failed to load nginx");
```

Then rebuild and run:

```bash
cargo build --release && \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

Test from host:

```bash
curl http://127.0.0.1:18080/
# Expected: <html><body><h1>Hello from RVOS nginx!</h1></body></html>
```

## Syscalls Implemented

| # | Name | Status | Notes |
|---|------|--------|-------|
| 17 | `getcwd` | ✅ | Hardcoded `/tmp` |
| 19 | `eventfd2` | ✅ | Fake fd |
| 20–22 | `epoll_create1/ctl/pwait` | ✅ | Bridged to smoltcp socket polling |
| 23–24 | `dup2/dup3` | ✅ | |
| 25 | `fcntl` | ✅ | `F_GETFD/SETFD/GETFL/SETFL` |
| 29 | `ioctl` | ✅ | `FIONBIO` only; terminal ioctls return `-1` |
| 34 | `mkdirat` | ✅ | RAMFS only |
| 48 | `faccessat` | ✅ | Returns `-ENOENT` (`-2`) for missing files |
| 54 | `fchownat` | ✅ | Stub (returns 0) |
| 56 | `openat` | ✅ | Returns `-ENOENT` (`-2`) for missing files |
| 57 | `close` | ✅ | Refuses to close stdio fds (0, 1, 2) |
| 59 | `pipe2` | ✅ | Ring-buffer pipe; `fork` shares pipe fd |
| 61 | `getdents64` | ✅ | RAMFS directory listing with `.` / `..` |
| 62 | `lseek` | ✅ | |
| 63–64 | `read/write` | ✅ | Files + pipes + sockets + NSCD + UART stdin |
| 66 | `writev` | ✅ | Scatter-gather write |
| 67–68 | `pread64/pwrite64` | ✅ | |
| 71 | `sendfile` | ✅ | Read/write fallback |
| 73 | `ppoll` | ✅ | Fake fds only |
| 78 | `readlinkat` | ✅ | `/proc/self/exe` → `/sbin/nginx` |
| 79–80 | `fstatat/fstat` | ✅ | Minimal 128-byte stat; stdio returns `S_IFCHR` |
| 93–94 | `exit/exit_group` | ✅ | Marks Zombie for `wait4`; wakes waiting parent |
| 96 | `set_tid_address` | ✅ | Stub |
| 99 | `set_robust_list` | ✅ | Stub |
| 113 | `clock_gettime` | ✅ | `rdtime` at 10 MHz |
| 122–123 | `sched_set/getaffinity` | ✅ | Stub |
| 134–135 | `rt_sigaction/rt_sigprocmask` | ✅ | Stub |
| 160 | `uname` | ✅ | Reports Linux/rvos/riscv64 |
| 163 | `getrlimit` | ✅ | `NOFILE=1024` |
| 166 | `umask` | ✅ | Returns `022` |
| 167 | `prctl` | ✅ | Stub |
| 172–177 | `getpid/ppid/uid/euid/gid/egid` | ✅ | Real pid via `CURRENT_PID` |
| 198–210 | Socket family | ✅ | `socket/bind/listen/accept/accept4/connect/getsockname/getpeername/sendto/recvfrom/setsockopt/getsockopt/shutdown` |
| 212 | `recvmsg` | ✅ | NSCD only |
| 214 | `brk` | ✅ | Per-process, page-aligned, contiguous with ELF |
| 215 | `munmap` | ⚠️ | Stub (returns 0) |
| 220 | `clone` | ✅ | Fork with shared fd table / page-table clone |
| 221 | `execve` | ✅ | Replaces address space, preserves pid/fds; returns `-ENOENT` (`-2`) on failure |
| 222 | `mmap` | ✅ | Anonymous + file-backed, `MAP_FIXED` |
| 226 | `mprotect` | ⚠️ | Stub |
| 233 | `madvise` | ⚠️ | Stub |
| 260 | `wait4` | ✅ | Blocks until child exits; supports `WNOHANG` |
| 261 | `prlimit64` | ✅ | `NOFILE` only |
| 278 | `getrandom` | ✅ | Returns `0xAB` pattern |
| 291 | `statx` | ✅ | |

## Networking

The kernel runs nginx with a fixed virtio-net configuration:

- **IP**: `10.0.2.15/24`
- **Gateway**: `10.0.2.2`
- **MAC**: QEMU-assigned or `52:54:00:12:34:56`
- **Listen port**: 80 (forwarded to host `18080`)

ARP, ICMP ping, and TCP (HTTP) are all functional. The network stack uses a busy-wait `epoll_pwait` loop — there are no timer interrupts or preemptive scheduling.

## Filesystem Layout (RAMFS)

At boot, `fs::init()` creates a minimal Linux-like filesystem tree:

```
/bin/busybox, /bin/sh, /bin/ls, /bin/cat, /bin/echo, /bin/pwd, /bin/mkdir, /bin/ps
/sbin/nginx                    — nginx binary (embedded at compile time)
/etc/nginx/nginx.conf          — nginx config
/usr/local/nginx/conf/nginx.conf
/usr/local/nginx/html/index.html
/var/log/nginx/error.log
/proc/stat, /proc/cpuinfo
/sys/devices/system/cpu/online
/etc/passwd, /etc/group, /etc/nsswitch.conf
/lib/riscv64-linux-gnu/libnss_files.so.2
```

All files live in a `BTreeMap<String, File>` guarded by a `spin::Mutex`. Static files reference embedded byte slices; dynamic files (logs) use `Vec<u8>` and support `write`/`lseek`.

## Memory Model

- **Kernel heap**: Bump allocator at `0x8600_0000`–`0x8700_0000` (16 MiB)
- **Page frames**: Allocated from `kernel_end` up to `0x8800_0000`
- **Page size**: 4 KiB, SV39 three-level page tables
- **User virtual space**: Lower half (`0x0000_0000_0000_0000`–`0x0000_003f_ffff_ffff`)
- **Kernel identity mapping**: `0x8000_0000`–`0x8800_0000`, UART at `0x1000_0000`, virtio-mmio at `0x1000_1000`–`0x1000_8000`, CLINT at `0x0c00_0000`
- **User stack**: 8 pages at top of lower half (`0x3f_ffff_f000` downward)
- **User brk**: Initialized to page-aligned end of highest ELF `PT_LOAD` segment (contiguous heap, matching Linux behavior)

## Process Loading

`proc::init_user_proc` performs the following steps:

1. Create new SV39 page table and 8 KiB kernel stack
2. Identity-map kernel and device regions
3. Allocate and map user stack (9 pages including guard)
4. Write initial stack layout: `argc`, `argv[]`, `envp[]`, `auxv[]`
5. Populate `AT_PAGESZ`, `AT_PHDR`, `AT_PHENT`, `AT_PHNUM`, `AT_ENTRY`, `AT_RANDOM`, `AT_CLKTCK`
6. Copy ELF to aligned kernel buffer for safe parsing
7. Parse program headers, map each `PT_LOAD` segment with correct R/W/X flags and zero BSS
8. Set `proc.brk` to page-aligned max segment end
9. Configure `sepc` = entry point, `sstatus.SPIE=1`, `sstatus.SPP=0`, `sp` = user stack top
10. Enter user mode via `sret`

## Known Limitations

1. **Cooperative scheduling only** — Round-robin on syscall traps; no timer interrupts or preemption.
2. **No block device** — Although VirtIO block init exists, there is no persistent storage. All data is in RAMFS.
3. **Bump heap only** — No `free` for kernel heap allocations; page frames are also never freed.
4. **No signal delivery** — `rt_sigaction`/`rt_sigprocmask` are stubs.
5. **getrandom is not random** — Returns a fixed `0xAB` pattern.
6. **UART is polled only** — `sys_read` from stdin busy-waits on `console::getchar()`; no DMA or interrupt-driven input.

## Debug Tips

View kernel logs while QEMU runs:

```bash
tail -f /tmp/qemu.log
```

Rebuild and restart in one shot:

```bash
cargo build --release && \
pkill -9 qemu-system-riscv64 && \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

## License

This is an educational operating system. Use it however you like.

---

# RVOS — 用 Rust 编写的 RISC-V 操作系统（中文版）

一个极简的 RISC-V 64 位操作系统内核，使用 Rust 编写（`#![no_std]`）。它能够在 QEMU 中通过 virtio-net 网络运行未经修改的静态链接 Linux 二进制程序（busybox shell 和 nginx）。

## 功能特性

- **RISC-V 64** (`rv64gc`) 裸机内核，支持 SV39 虚拟内存
- **VirtIO-MMIO** virtio-net 设备驱动
- **smoltcp 0.11** TCP/IP 协议栈，完整模拟 socket 相关系统调用（`socket`、`bind`、`listen`、`accept`、`sendto`、`recvfrom`）
- **Linux 系统调用兼容层** — 模拟约 50 个系统调用，使未经修改的 glibc 静态二进制程序无需重新编译即可运行
- **多进程支持** — `clone`/`fork`、`execve`、`wait4`、`exit`，以及基于系统调用陷阱的轮询协作式调度，支持阻塞等待
- **Shell 与管道** — busybox `sh` 支持 `fork`+`execve`、管道 `pipe2`（`cmd1 | cmd2`）以及批处理模式读取命令
- **内存文件系统 RAMFS** — 支持静态文件、动态文件和 `getdents64` 目录遍历
- **ELF 加载** — 完整映射 `PT_LOAD` 段、初始化 auxv、支持每进程独立的 `brk`
- **用户/内核模式切换** — 通过 S-mode trap handler 完整保存和恢复所有寄存器

## 系统架构

```
+-------------------------------------------+
|  用户空间：busybox sh, nginx, cat, ls    |
+-------------------------------------------+
|  Linux 系统调用模拟层 (src/syscall.rs)   |
+-------------------------------------------+
|  进程与 ELF      |  网络协议栈 (smoltcp) |
|  (src/proc.rs)   |  (src/net.rs)         |
+-------------------------------------------+
|  Trap 处理程序   |  VirtIO-Net 驱动      |
|  (src/trap.rs)   |  (src/virtio.rs)      |
+-------------------------------------------+
|  内存管理（SV39 页表）                    |
|  (src/mm.rs)                              |
+-------------------------------------------+
|  UART 控制台 | SBI | RAMFS (src/fs.rs)   |
+-------------------------------------------+
```

### 模块说明

| 文件 | 职责 |
|------|------|
| `entry.asm` | 启动入口：清零 BSS、设置栈、调用 `rust_main` |
| `linker.ld` | 内核链接地址 `0x8020_0000` |
| `main.rs` | 初始化流程：console → mm → heap → page table → trap → virtio → net → fs → proc → 运行 busybox shell |
| `console.rs` | UART 输出（`print`）和输入（`getchar`），通过 16550 寄存器直接访问 |
| `mm.rs` | Bump 堆分配器、页框分配器、SV39 `PageTable`（含 `map`/`translate`） |
| `trap.rs` | 裸 trap 向量（汇编）：切换到内核栈、保存所有寄存器、设置 `sstatus.SUM`、调用 `rust_trap_handler`、恢复并执行 `sret` |
| `virtio.rs` | VirtIO-MMIO 传输层 + `VirtIONet` 驱动（基于 `virtio-drivers` crate） |
| `net.rs` | `smoltcp` 的 `Device` 实现，桥接 virtio-net 的 RX/TX；socket fd 管理、TCP listen/accept/send/recv |
| `fs.rs` | 基于 BTreeMap 的内存文件系统；`FileContents::Static` 用于内嵌二进制数据，`FileContents::Dynamic` 用于可写日志 |
| `proc.rs` | `Process` 结构体，包含 SV39 页表、`TrapFrame`、内核栈、每进程独立的 `brk`；带 auxv 设置的 ELF 加载器 |
| `syscall.rs` | 约 50 个 Linux 系统调用的分发器；fd 表、管道、epoll/eventfd/NSCD 的 fake fd、socket 模拟 |

## 编译构建

需要安装 Rust nightly 工具链，并添加 `riscv64gc-unknown-none-elf` 目标：

```bash
rustup target add riscv64gc-unknown-none-elf
cargo build --release
```

**注意**：内核在编译时通过 `include_bytes!` 内嵌预编译的静态二进制文件：
- `/tmp/busybox-1.36.1/busybox` → `/bin/sh`、`/bin/ls`、`/bin/cat` 等
- `/tmp/nginx-1.26.3/objs/nginx` → `/sbin/nginx`
- `../libnss_files.so.2` → `/lib/riscv64-linux-gnu/libnss_files.so.2`

如果你修改了这些二进制文件，必须重新编译内核才能生效。

## 运行

### 默认：busybox shell

内核启动后进入 busybox `sh`，从 stdin 读取命令（批处理模式，`isatty()` 返回 false）。

```bash
qemu-system-riscv64 \
  -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

通过管道发送命令：

```bash
(echo "ls /"; echo "exit") | \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

期望输出：

```
bin       lib       sbin      test_malloc  usr
etc       proc      sys       tmp          var
```

如果命令发送得太早，第一个字符可能会因为 UART 初始化时序而丢失。建议加 2~3 秒延迟：

```bash
(sleep 3; echo "ls /"; sleep 2; echo "exit") | \
qemu-system-riscv64 ...
```

### 运行 nginx

编辑 `src/main.rs`，将 shell 启动代码替换为 nginx：

```rust
// let sh_data = fs::get_file_data("/bin/sh").expect("sh binary not found");
// proc::init_user_proc(sh_data, &["sh"]).expect("failed to load shell");

let nginx_data = fs::get_file_data("/bin/nginx").expect("nginx binary not found");
proc::init_user_proc(nginx_data, &["nginx"]).expect("failed to load nginx");
```

然后重新编译并运行：

```bash
cargo build --release && \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

在宿主机上测试：

```bash
curl http://127.0.0.1:18080/
# 期望输出：<html><body><h1>Hello from RVOS nginx!</h1></body></html>
```

## 已实现的系统调用

| 编号 | 名称 | 状态 | 说明 |
|------|------|------|------|
| 17 | `getcwd` | ✅ | 固定返回 `/tmp` |
| 19 | `eventfd2` | ✅ | Fake fd |
| 20–22 | `epoll_create1/ctl/pwait` | ✅ | 桥接到 smoltcp socket 轮询 |
| 23–24 | `dup2/dup3` | ✅ | |
| 25 | `fcntl` | ✅ | 仅支持 `F_GETFD/SETFD/GETFL/SETFL` |
| 29 | `ioctl` | ✅ | 仅 `FIONBIO`；终端 ioctl 返回 `-1` |
| 34 | `mkdirat` | ✅ | 仅 RAMFS |
| 48 | `faccessat` | ✅ | 文件不存在时返回 `-ENOENT` (`-2`) |
| 54 | `fchownat` | ✅ | Stub（直接返回 0） |
| 56 | `openat` | ✅ | 文件不存在时返回 `-ENOENT` (`-2`) |
| 57 | `close` | ✅ | 拒绝关闭 stdio fd（0, 1, 2） |
| 59 | `pipe2` | ✅ | 环形缓冲区管道；`fork` 共享 pipe fd |
| 61 | `getdents64` | ✅ | RAMFS 目录遍历，含 `.` / `..` |
| 62 | `lseek` | ✅ | |
| 63–64 | `read/write` | ✅ | 支持文件、管道、socket、NSCD、UART 标准输入 |
| 66 | `writev` | ✅ | 分散/聚集写 |
| 67–68 | `pread64/pwrite64` | ✅ | |
| 71 | `sendfile` | ✅ | 读/写回退 |
| 73 | `ppoll` | ✅ | 仅 fake fd |
| 78 | `readlinkat` | ✅ | `/proc/self/exe` 返回 `/sbin/nginx` |
| 79–80 | `fstatat/fstat` | ✅ | 最小 128 字节 stat 结构；stdio 返回 `S_IFCHR` |
| 93–94 | `exit/exit_group` | ✅ | 标记 Zombie 供 `wait4` 回收；唤醒等待中的父进程 |
| 96 | `set_tid_address` | ✅ | Stub |
| 99 | `set_robust_list` | ✅ | Stub |
| 113 | `clock_gettime` | ✅ | 基于 `rdtime`，时钟频率 10 MHz |
| 122–123 | `sched_set/getaffinity` | ✅ | Stub |
| 134–135 | `rt_sigaction/rt_sigprocmask` | ✅ | Stub |
| 160 | `uname` | ✅ | 报告 Linux/rvos/riscv64 |
| 163 | `getrlimit` | ✅ | `NOFILE=1024` |
| 166 | `umask` | ✅ | 返回 `022` |
| 167 | `prctl` | ✅ | Stub |
| 172–177 | `getpid/ppid/uid/euid/gid/egid` | ✅ | 真实 pid（通过 `CURRENT_PID`） |
| 198–210 | Socket 系列 | ✅ | `socket/bind/listen/accept/accept4/connect/getsockname/getpeername/sendto/recvfrom/setsockopt/getsockopt/shutdown` |
| 212 | `recvmsg` | ✅ | 仅 NSCD |
| 214 | `brk` | ✅ | 每进程独立、页对齐、与 ELF 最高段地址连续 |
| 215 | `munmap` | ⚠️ | Stub（返回 0） |
| 220 | `clone` | ✅ | 复制页表和 fd 表的 fork |
| 221 | `execve` | ✅ | 替换地址空间，保留 pid/fd；失败时返回 `-ENOENT` (`-2`) |
| 222 | `mmap` | ✅ | 支持匿名映射和文件映射、`MAP_FIXED` |
| 226 | `mprotect` | ⚠️ | Stub |
| 233 | `madvise` | ⚠️ | Stub |
| 260 | `wait4` | ✅ | 阻塞直到子进程退出；支持 `WNOHANG` |
| 261 | `prlimit64` | ✅ | 仅 `NOFILE` |
| 278 | `getrandom` | ✅ | 返回固定模式 `0xAB` |
| 291 | `statx` | ✅ | |

## 网络

内核使用固定的 virtio-net 配置运行 nginx：

- **IP**：`10.0.2.15/24`
- **网关**：`10.0.2.2`
- **MAC**：由 QEMU 分配，或固定为 `52:54:00:12:34:56`
- **监听端口**：80（映射到宿主机的 `18080`）

ARP、ICMP ping 和 TCP（HTTP）均正常工作。网络栈使用 busy-wait 的 `epoll_pwait` 循环 — 没有定时器中断，也没有抢占式调度。

## 文件系统布局（RAMFS）

启动时，`fs::init()` 会创建一个最小化的类 Linux 文件系统树：

```
/bin/busybox, /bin/sh, /bin/ls, /bin/cat, /bin/echo, /bin/pwd, /bin/mkdir, /bin/ps
/sbin/nginx                    — nginx 二进制文件（编译时内嵌）
/etc/nginx/nginx.conf          — nginx 配置文件
/usr/local/nginx/conf/nginx.conf
/usr/local/nginx/html/index.html
/var/log/nginx/error.log
/proc/stat, /proc/cpuinfo
/sys/devices/system/cpu/online
/etc/passwd, /etc/group, /etc/nsswitch.conf
/lib/riscv64-linux-gnu/libnss_files.so.2
```

所有文件存储在受 `spin::Mutex` 保护的 `BTreeMap<String, File>` 中。静态文件引用内嵌的字节切片；动态文件（日志）使用 `Vec<u8>`，支持 `write`/`lseek`。

## 内存模型

- **内核堆**：Bump 分配器，地址范围 `0x8600_0000`–`0x8700_0000`（16 MiB）
- **页框**：从 `kernel_end` 到 `0x8800_0000` 之间分配
- **页大小**：4 KiB，SV39 三级页表
- **用户虚拟空间**：下半地址空间（`0x0000_0000_0000_0000`–`0x0000_003f_ffff_ffff`）
- **内核恒等映射**：`0x8000_0000`–`0x8800_0000`，UART 在 `0x1000_0000`，virtio-mmio 在 `0x1000_1000`–`0x1000_8000`，CLINT 在 `0x0c00_0000`
- **用户栈**：下半地址空间顶部向下 8 页（从 `0x3f_ffff_f000` 开始）
- **用户 brk**：初始化为 ELF 最高 `PT_LOAD` 段的页对齐结束地址（与 Linux 行为一致，保证堆内存连续）

## 进程加载流程

`proc::init_user_proc` 按以下步骤加载用户程序：

1. 创建新的 SV39 页表和 8 KiB 内核栈
2. 恒等映射内核和设备区域
3. 分配并映射用户栈（含 guard 共 9 页）
4. 写入初始栈布局：`argc`、`argv[]`、`envp[]`、`auxv[]`
5. 填充 `AT_PAGESZ`、`AT_PHDR`、`AT_PHENT`、`AT_PHNUM`、`AT_ENTRY`、`AT_RANDOM`、`AT_CLKTCK`
6. 将 ELF 复制到内核的对齐缓冲区以便安全解析
7. 解析 program headers，用正确的 R/W/X 标志映射每个 `PT_LOAD` 段，并将 BSS 段清零
8. 将 `proc.brk` 设为最高段的页对齐结束地址
9. 配置 `sepc` = 入口地址、`sstatus.SPIE=1`、`sstatus.SPP=0`、`sp` = 用户栈顶
10. 通过 `sret` 进入用户模式

## 已知限制

1. **仅协作式调度** — 内核是协作式的；用户进程一直运行直到触发 trap（系统调用或异常）。没有定时器中断或抢占式调度。
2. **无块设备** — 虽然存在 VirtIO block 初始化代码，但没有持久化存储。所有数据都在 RAMFS 中。
3. **Bump 堆分配器** — 内核堆分配不支持 `free`；页框也永不释放。
4. **无信号投递** — `rt_sigaction`/`rt_sigprocmask` 是 stub。
5. **getrandom 不是真随机** — 返回固定的 `0xAB` 模式。
6. **UART 仅轮询** — `sys_read` 从 stdin 读取时忙等待 `console::getchar()`；没有 DMA 或中断驱动输入。

## 调试技巧

实时查看内核日志：

```bash
tail -f /tmp/qemu.log
```

一键重新编译并启动：

```bash
cargo build --release && \
pkill -9 qemu-system-riscv64 && \
qemu-system-riscv64 -machine virt -m 512M -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/rvos \
  -netdev user,id=net0,hostfwd=tcp::18080-:80 \
  -device virtio-net-device,netdev=net0
```

## 许可证

本项目是教育性质的操作系统。您可以自由使用。
