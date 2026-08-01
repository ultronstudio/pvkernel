use core::fmt::{self, Write};
use spin::{Mutex, Once};
use uart_16550::SerialPort;

const COM1_PORT: u16 = 0x3f8;

static SERIAL: Once<Mutex<SerialPort>> = Once::new();

fn serial() -> &'static Mutex<SerialPort> {
    SERIAL.call_once(|| {
        // SAFETY: 0x3f8 is the standard COM1 I/O port address on the x86_64
        // platforms pvKernel targets. This closure runs at most once (enforced
        // by `Once`), so no other code constructs a competing `SerialPort` for
        // the same I/O port.
        let mut port = unsafe { SerialPort::new(COM1_PORT) };
        port.init();
        Mutex::new(port)
    })
}

/// Writes formatted output to COM1.
///
/// If the serial lock is already held by the calling context (for example a
/// panic or exception handler firing while a normal `kprintln!` is in
/// progress), the write is silently skipped instead of spinning forever.
/// This keeps logging non-reentrant call sites safe without ever deadlocking.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    if let Some(mut guard) = serial().try_lock() {
        let _ = guard.write_fmt(args);
    }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::drivers::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::kprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::drivers::serial::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
