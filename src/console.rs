use core::fmt::{self, Write};
use spin::Mutex;

const UART_BASE: usize = 0x1000_0000;

struct Uart;

static UART: Mutex<Uart> = Mutex::new(Uart);

impl Uart {
    fn putchar(&self, c: u8) {
        let ptr = UART_BASE as *mut u8;
        unsafe {
            ptr.write_volatile(c);
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            self.putchar(c);
        }
        Ok(())
    }
}

pub fn init() {
    // Nothing to do for QEMU's UART
}

pub fn print(args: fmt::Arguments) {
    UART.lock().write_fmt(args).unwrap();
}

/// Try to read a byte from UART. Returns None if no data available.
pub fn getchar() -> Option<u8> {
    const LSR: usize = UART_BASE + 5;
    unsafe {
        let lsr = (LSR as *const u8).read_volatile();
        if lsr & 1 != 0 {
            Some((UART_BASE as *const u8).read_volatile())
        } else {
            None
        }
    }
}
