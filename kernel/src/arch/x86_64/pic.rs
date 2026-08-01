//! Legacy dual 8259 Programmable Interrupt Controller (PIC) driver.
//!
//! This is a temporary early x86_64 foundation, useful for QEMU, learning the
//! interrupt path, and simple compatible systems. It is intentionally kept
//! small and behind this module boundary so it can later be replaced by an
//! APIC/IOAPIC driver without disturbing unrelated kernel code.
//!
//! pvKernel is single-core: nothing in this module synchronizes concurrent
//! access to the PIC's I/O ports. This is safe only because PIC
//! (re)programming happens with maskable interrupts disabled, and every
//! other access (masking, EOI) runs from a single CPU with no concurrent
//! caller. SMP synchronization is not provided.

use core::sync::atomic::{AtomicU16, Ordering};

use x86_64::instructions::port::{Port, PortWriteOnly};

/// IDT vector the master PIC's IRQ0 is remapped to.
pub const MASTER_OFFSET: u8 = 32;
/// IDT vector the slave PIC's IRQ8 is remapped to.
pub const SLAVE_OFFSET: u8 = 40;

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

/// Unused legacy diagnostic port, used only for its I/O bus delay between PIC
/// initialization writes. Not exposed outside this module.
const IO_WAIT_PORT: u16 = 0x80;

const ICW1_INIT_WITH_ICW4: u8 = 0x11;
const ICW4_8086: u8 = 0x01;
/// Told to the master: a slave PIC is wired to IRQ2.
const MASTER_CASCADE_IRQ_BIT: u8 = 0x04;
/// Told to the slave: its cascade identity is 2.
const SLAVE_CASCADE_IDENTITY: u8 = 0x02;

const MASK_ALL: u8 = 0xFF;

const EOI: u8 = 0x20;
/// OCW3 command selecting the in-service register for the next read.
const OCW3_READ_ISR: u8 = 0x0B;

/// One of the 16 legacy PIC IRQ lines.
///
/// This identifies the IRQ *line*, not the IDT vector it is remapped to; use
/// [`IrqLine::vector`] to obtain the vector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IrqLine {
    Timer = 0,
    Keyboard = 1,
    Cascade = 2,
    Com2 = 3,
    Com1 = 4,
    Lpt2 = 5,
    Floppy = 6,
    Lpt1 = 7,
    RealTimeClock = 8,
    Free9 = 9,
    Free10 = 10,
    Free11 = 11,
    Mouse = 12,
    FpuCoprocessor = 13,
    PrimaryAta = 14,
    SecondaryAta = 15,
}

impl IrqLine {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// The IDT vector this IRQ line is remapped to (32-47).
    pub const fn vector(self) -> u8 {
        MASTER_OFFSET + self.as_u8()
    }

    /// Whether this IRQ line is served by the slave PIC (IRQ8-15).
    const fn is_slave(self) -> bool {
        self.as_u8() >= 8
    }

    /// This IRQ line's bit position within its own PIC's 8-bit mask/ISR
    /// register (0-7 on both the master and the slave).
    pub const fn bit(self) -> u8 {
        1 << (self.as_u8() % 8)
    }
}

fn master_command() -> Port<u8> {
    Port::new(MASTER_COMMAND)
}

fn master_data() -> Port<u8> {
    Port::new(MASTER_DATA)
}

fn slave_command() -> Port<u8> {
    Port::new(SLAVE_COMMAND)
}

fn slave_data() -> Port<u8> {
    Port::new(SLAVE_DATA)
}

fn io_wait() {
    // SAFETY: 0x80 is an unused legacy diagnostic port on the platforms
    // pvKernel targets; the write's only effect used here is the I/O bus
    // delay it takes to complete, giving the PIC time to process the
    // preceding command during the ICW sequence below.
    unsafe {
        PortWriteOnly::<u8>::new(IO_WAIT_PORT).write(0);
    }
}

/// Records IRQ lines that delivered a real (non-spurious) interrupt but have
/// no dedicated handler logic beyond acknowledging them. Read from normal
/// kernel context only; `Relaxed` is sufficient because this is a simple
/// observational bitset, not a synchronization point for other state.
static UNEXPECTED_IRQS: AtomicU16 = AtomicU16::new(0);

/// Marks `irq` as having delivered an unexpected real interrupt.
pub fn record_unexpected(irq: IrqLine) {
    UNEXPECTED_IRQS.fetch_or(1 << irq.as_u8(), Ordering::Relaxed);
}

/// Returns the current unexpected-IRQ bitset without clearing it.
///
/// Part of the documented driver API, symmetric with
/// [`take_unexpected_irq_mask`]; 0.0.4's own boot verification only needs the
/// clearing variant, so there is no current caller.
#[allow(dead_code)]
pub fn unexpected_irq_mask() -> u16 {
    UNEXPECTED_IRQS.load(Ordering::Relaxed)
}

/// Returns the current unexpected-IRQ bitset and clears it.
pub fn take_unexpected_irq_mask() -> u16 {
    UNEXPECTED_IRQS.swap(0, Ordering::Relaxed)
}

/// Initializes and remaps both legacy 8259 PICs.
///
/// Must run with maskable hardware interrupts already disabled and before
/// any IRQ line is unmasked. This leaves the mask registers in an
/// unspecified state (the last ICW4 write happens to land in them); callers
/// must follow up with [`mask_all`] before relying on any mask value. This
/// is safe only because maskable interrupts remain disabled for the whole
/// window between the two calls, so the CPU cannot latch a PIC-signaled
/// interrupt regardless of the mask register's contents.
pub fn initialize() {
    crate::kprintln!("[irq] remapping 8259 PIC to vectors 32-47");

    // SAFETY: this reprograms both 8259 PICs through their documented
    // ICW1-ICW4 port protocol. The caller (`arch::x86_64::init`) disables
    // maskable interrupts before this runs, and pvKernel is single-core, so
    // no other code accesses these ports concurrently during the sequence.
    unsafe {
        let mut master_cmd = master_command();
        let mut master_data = master_data();
        let mut slave_cmd = slave_command();
        let mut slave_data = slave_data();

        // ICW1: start initialization, expect an ICW4 write.
        master_cmd.write(ICW1_INIT_WITH_ICW4);
        io_wait();
        slave_cmd.write(ICW1_INIT_WITH_ICW4);
        io_wait();

        // ICW2: vector offsets, outside the CPU exception range 0-31.
        master_data.write(MASTER_OFFSET);
        io_wait();
        slave_data.write(SLAVE_OFFSET);
        io_wait();

        // ICW3: cascade wiring between master and slave.
        master_data.write(MASTER_CASCADE_IRQ_BIT);
        io_wait();
        slave_data.write(SLAVE_CASCADE_IDENTITY);
        io_wait();

        // ICW4: 8086/88 mode.
        master_data.write(ICW4_8086);
        io_wait();
        slave_data.write(ICW4_8086);
        io_wait();
    }

    crate::kprintln!("[irq] PIC initialized");
}

/// Masks every IRQ line on both PICs.
pub fn mask_all() {
    // SAFETY: writing the data (mask) ports is the documented way to set the
    // 8259 interrupt mask register; both ports were already programmed into
    // OCW1 mode by the ICW4 write in `initialize`.
    unsafe {
        master_data().write(MASK_ALL);
        slave_data().write(MASK_ALL);
    }
}

fn data_port_for(irq: IrqLine) -> Port<u8> {
    if irq.is_slave() {
        slave_data()
    } else {
        master_data()
    }
}

/// Masks (disables) a single IRQ line without affecting any other line.
///
/// Part of the documented driver API, symmetric with [`unmask`]; 0.0.4 never
/// re-masks a line after boot, so there is no current caller.
#[allow(dead_code)]
pub fn mask(irq: IrqLine) {
    let mut port = data_port_for(irq);
    // SAFETY: read-modify-write of a single PIC's own mask register; only
    // the bit for `irq` is changed, and mask bit 1 means "disabled".
    unsafe {
        let current = port.read();
        port.write(current | irq.bit());
    }
}

/// Unmasks (enables) a single IRQ line without affecting any other line.
pub fn unmask(irq: IrqLine) {
    let mut port = data_port_for(irq);
    // SAFETY: read-modify-write of a single PIC's own mask register; only
    // the bit for `irq` is changed, and mask bit 0 means "enabled".
    unsafe {
        let current = port.read();
        port.write(current & !irq.bit());
    }
}

/// Sends the end-of-interrupt command for `irq`.
///
/// For an IRQ served by the slave PIC (IRQ8-15), the slave must be
/// acknowledged before the master, since the master only saw the cascaded
/// IRQ2 line.
pub fn notify_end_of_interrupt(irq: IrqLine) {
    // SAFETY: writing 0x20 (EOI) to a PIC's command port is the documented
    // way to acknowledge the interrupt currently being serviced; this must
    // run from the corresponding IRQ's own handler context, which is the
    // only caller of this function.
    unsafe {
        if irq.is_slave() {
            slave_command().write(EOI);
        }
        master_command().write(EOI);
    }
}

/// Reads the in-service register of the PIC that owns `command_port`.
fn read_isr(mut command_port: Port<u8>) -> u8 {
    // SAFETY: OCW3 with the "read ISR" bit set selects the in-service
    // register for the next read from the same command port; this does not
    // alter the PIC's interrupt-handling state.
    unsafe {
        command_port.write(OCW3_READ_ISR);
        command_port.read()
    }
}

/// Reads the master PIC's in-service register.
pub fn master_in_service() -> u8 {
    read_isr(master_command())
}

/// Reads the slave PIC's in-service register.
pub fn slave_in_service() -> u8 {
    read_isr(slave_command())
}
