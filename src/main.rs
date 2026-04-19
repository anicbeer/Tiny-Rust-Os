#![no_std]
#![no_main]

extern crate alloc;

mod console;
mod sbi;
mod mm;
mod trap;
mod syscall;
mod proc;
mod virtio;
mod net;
mod fs;

use core::arch::global_asm;
use log::info;

global_asm!(include_str!("entry.asm"));

extern "C" {
    fn kernel_start();
    fn kernel_end();
}

#[no_mangle]
extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    // Very early UART init so we can print
    console::init();
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);

    info!("RVOS booting on hart {}", hartid);
    info!("Kernel: [{:#x}, {:#x})", kernel_start as usize, kernel_end as usize);
    info!("DTB at {:#x}", dtb);

    mm::init();
    mm::init_heap();
    mm::init_kernel_page_table();
    trap::init();
    virtio::init();
    net::init();
    fs::init();

    info!("Kernel init complete.");

    // Load and run busybox shell
    let sh_data = fs::get_file_data("/bin/sh").expect("sh binary not found");
    proc::init_user_proc(sh_data, &["sh", "-c", "echo '=== RVOS Shell ==='; ls /; cat /etc/passwd; echo '=== Done ==='"]).expect("failed to load shell");
    proc::run_user();
}

static LOGGER: SimpleLogger = SimpleLogger;

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        console::print(format_args!("[{}] {}\n", record.level(), record.args()));
    }
    fn flush(&self) {}
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    console::print(format_args!("PANIC: {}\n", info));
    loop {}
}
