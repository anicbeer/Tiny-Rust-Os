use core::arch::{asm, naked_asm};
use core::sync::atomic::{AtomicUsize, Ordering};

pub static LAST_TP: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub fregs: [usize; 32],
    pub satp: usize,
    pub sepc: usize,
    pub sstatus: usize,
    pub scause: usize,
    pub stval: usize,
}

impl TrapFrame {
    pub const fn new() -> Self {
        Self {
            regs: [0; 32],
            fregs: [0; 32],
            satp: 0,
            sepc: 0,
            sstatus: 0,
            scause: 0,
            stval: 0,
        }
    }
}

pub fn init() {
    extern "C" { fn trap_vector(); }
    unsafe {
        asm!("csrw stvec, {}", in(reg) trap_vector as usize);
    }
}

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text"]
unsafe extern "C" fn trap_vector() {
    naked_asm!(
        ".align 2",
        // Swap sp with sscratch to get kernel stack; sscratch now holds user sp
        "csrrw sp, sscratch, sp",
        // Allocate 288 bytes: x0..x31 (256), orig_sp (8), sepc (8), sstatus (8), padding to 16-align -> 288
        "addi sp, sp, -288",
        "sd x0, 0(sp)",
        "sd x1, 8(sp)",
        "sd x2, 16(sp)",
        "sd x3, 24(sp)",
        "sd x4, 32(sp)",
        "sd x5, 40(sp)",
        "sd x6, 48(sp)",
        "sd x7, 56(sp)",
        "sd x8, 64(sp)",
        "sd x9, 72(sp)",
        "sd x10, 80(sp)",
        "sd x11, 88(sp)",
        "sd x12, 96(sp)",
        "sd x13, 104(sp)",
        "sd x14, 112(sp)",
        "sd x15, 120(sp)",
        "sd x16, 128(sp)",
        "sd x17, 136(sp)",
        "sd x18, 144(sp)",
        "sd x19, 152(sp)",
        "sd x20, 160(sp)",
        "sd x21, 168(sp)",
        "sd x22, 176(sp)",
        "sd x23, 184(sp)",
        "sd x24, 192(sp)",
        "sd x25, 200(sp)",
        "sd x26, 208(sp)",
        "sd x27, 216(sp)",
        "sd x28, 224(sp)",
        "sd x29, 232(sp)",
        "sd x30, 240(sp)",
        "sd x31, 248(sp)",
        // save original user sp (now in sscratch)
        "csrr t0, sscratch",
        "sd t0, 256(sp)",
        "csrr t0, sepc",
        "sd t0, 264(sp)",
        "csrr t0, sstatus",
        "sd t0, 272(sp)",
        "li t0, 1 << 18",      // sstatus.SUM
        "csrs sstatus, t0",    // allow S-mode to access U-mode pages
        "mv a0, sp",
        "call rust_trap_handler",
        // Restore
        "ld x0, 0(sp)",   // no-op but keeps offsets
        "ld x1, 8(sp)",
        "ld x3, 24(sp)",
        "ld x4, 32(sp)",
        // x5 (t0) restored later
        "ld x6, 48(sp)",
        "ld x7, 56(sp)",
        "ld x8, 64(sp)",
        "ld x9, 72(sp)",
        "ld x10, 80(sp)",
        "ld x11, 88(sp)",
        "ld x12, 96(sp)",
        "ld x13, 104(sp)",
        "ld x14, 112(sp)",
        "ld x15, 120(sp)",
        "ld x16, 128(sp)",
        "ld x17, 136(sp)",
        "ld x18, 144(sp)",
        "ld x19, 152(sp)",
        "ld x20, 160(sp)",
        "ld x21, 168(sp)",
        "ld x22, 176(sp)",
        "ld x23, 184(sp)",
        "ld x24, 192(sp)",
        "ld x25, 200(sp)",
        "ld x26, 208(sp)",
        "ld x27, 216(sp)",
        "ld x28, 224(sp)",
        "ld x29, 232(sp)",
        "ld x30, 240(sp)",
        "ld x31, 248(sp)",
        "ld t0, 264(sp)",
        "csrw sepc, t0",
        "ld t0, 272(sp)",
        "csrw sstatus, t0",
        "addi t0, sp, 288",   // kernel stack top
        "csrw sscratch, t0",
        "ld t0, 40(sp)",      // restore original t0
        "ld sp, 256(sp)",     // restore user sp
        "sret",
    );
}

#[repr(C)]
struct TrapFrameRaw {
    x: [usize; 32],      // 0..256
    orig_sp: usize,      // 256
    sepc: usize,         // 264
    sstatus: usize,      // 272
}

#[no_mangle]
extern "C" fn rust_trap_handler(tf: &mut TrapFrameRaw) {
    let scause: usize;
    let stval: usize;
    let sepc: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) scause);
        asm!("csrr {}, stval", out(reg) stval);
        asm!("csrr {}, sepc", out(reg) sepc);
    }

    let cause = scause & !(1usize << 63);
    let from_user = (tf.sstatus & (1 << 8)) == 0; // SPP=0 means from U-mode

    log::info!("TRAP cause={} sepc={:#x} stval={:#x} from_user={}", cause, sepc, stval, from_user);

    match cause {
        8 | 9 | 10 => { // ecall from U/S/M
            tf.sepc += 4; // advance past ecall instruction
            let num = tf.x[17]; // a7
            let args = [tf.x[10], tf.x[11], tf.x[12], tf.x[13], tf.x[14], tf.x[15]];
            if num == 57 {
                LAST_TP.store(tf.x[4], Ordering::Relaxed);
                log::info!("SYSCALL {} args={:x?} sepc={:#x} ra={:#x} tp={:#x}", num, args, sepc, tf.x[1], tf.x[4]);
            } else {
                log::debug!("SYSCALL {} args={:x?} sepc={:#x}", num, args, sepc);
            }
            let ret = crate::syscall::dispatch(num, args);
            if num == 57 {
                log::info!("SYSCALL {} return {}", num, ret);
            } else {
                log::debug!("SYSCALL {} return {}", num, ret);
            }
            tf.x[10] = ret as usize; // a0
        }
        _ => {
            log::warn!("Trap cause={} sepc={:#x} stval={:#x} from_user={}", cause, sepc, stval, from_user);
            log::warn!("  ra={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x} sp={:#x}",
                       tf.x[1], tf.x[10], tf.x[11], tf.x[12], tf.x[13], tf.orig_sp);
            let mut dump = alloc::string::String::new();
            for i in 0..8 {
                let addr = tf.orig_sp + i * 8;
                let val = if let Some(proc) = crate::proc::CURRENT_PROC.lock().as_ref() {
                    proc.page_table.translate(addr).map(|pa| unsafe { *(pa as *const usize) }).unwrap_or(0)
                } else { 0 };
                dump.push_str(&alloc::format!(" {:#x}={:#x}", addr, val));
            }
            log::warn!(" stack:{}" , dump);
            loop {}
        }
    }
}
