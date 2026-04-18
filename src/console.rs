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
