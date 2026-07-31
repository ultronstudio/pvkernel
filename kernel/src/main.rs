#![no_std]
#![no_main]

use bootloader_api::{
    BootInfo, entry_point,
    info::{FrameBuffer, FrameBufferInfo, PixelFormat},
};
use core::{fmt::Write, panic::PanicInfo};
use uart_16550::SerialPort;
use x86_64::instructions::hlt;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    let mut serial = serial_port();

    let _ = writeln!(serial);
    let _ = writeln!(serial, "================================");
    let _ = writeln!(serial, "pvKernel 0.0.1");
    let _ = writeln!(serial, "Architecture: x86_64");
    let _ = writeln!(serial, "Boot mode: UEFI");
    let _ = writeln!(serial, "================================");

    let usable_memory: u64 = boot_info
        .memory_regions
        .iter()
        .filter(|region| region.kind == bootloader_api::info::MemoryRegionKind::Usable)
        .map(|region| region.end.saturating_sub(region.start))
        .sum();

    let _ = writeln!(
        serial,
        "Usable physical memory: {} MiB",
        usable_memory / 1024 / 1024
    );

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();

        let _ = writeln!(
            serial,
            "Framebuffer: {}x{} pixels, stride {}, {} B/px, {:?}",
            info.width, info.height, info.stride, info.bytes_per_pixel, info.pixel_format
        );

        render_boot_screen(framebuffer);

        let _ = writeln!(serial, "Framebuffer initialized");
    } else {
        let _ = writeln!(serial, "WARNING: No framebuffer available");
    }

    let _ = writeln!(serial, "pvKernel successfully entered kernel_main");
    let _ = writeln!(serial, "CPU halted. Close QEMU window to stop.");

    halt_loop();
}

fn render_boot_screen(framebuffer: &mut FrameBuffer) {
    let info = framebuffer.info();
    let buffer = framebuffer.buffer_mut();

    let background = [8, 12, 20];
    let accent = [0, 183, 239]; // #00B7EF

    // Vyplnit celé pozadí
    fill_rect(buffer, info, 0, 0, info.width, info.height, background);

    // Velikost loga/P
    let p_size = info.height.min(info.width).saturating_mul(35) / 100;
    let p_x = (info.width.saturating_sub(p_size)) / 2;
    let p_y = (info.height.saturating_sub(p_size)) / 2;

    draw_p_logo(buffer, info, p_x, p_y, p_size, accent);
}

fn draw_p_logo(
    buffer: &mut [u8],
    info: FrameBufferInfo,
    x: usize,
    y: usize,
    size: usize,
    color: [u8; 3],
) {
    let stroke = (size / 8).max(6);

    let stem_x = x + size / 5;
    let top_y = y + size / 8;

    let bowl_w = size / 2;
    let bowl_h = size / 3;

    // Levá svislá noha
    fill_rect(buffer, info, stem_x, top_y, stroke, size * 3 / 4, color);

    // Horní vodorovná čára
    fill_rect(buffer, info, stem_x, top_y, bowl_w, stroke, color);

    // Prostřední vodorovná čára
    fill_rect(buffer, info, stem_x, top_y + bowl_h, bowl_w, stroke, color);

    // Pravá svislá část horní smyčky
    fill_rect(
        buffer,
        info,
        stem_x + bowl_w - stroke,
        top_y,
        stroke,
        bowl_h + stroke,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    buffer: &mut [u8],
    info: FrameBufferInfo,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: [u8; 3],
) {
    let end_x = x.saturating_add(width).min(info.width);
    let end_y = y.saturating_add(height).min(info.height);

    for current_y in y.min(info.height)..end_y {
        for current_x in x.min(info.width)..end_x {
            write_pixel(buffer, info, current_x, current_y, color);
        }
    }
}

fn write_pixel(buffer: &mut [u8], info: FrameBufferInfo, x: usize, y: usize, color: [u8; 3]) {
    if x >= info.width || y >= info.height {
        return;
    }

    let bytes_per_pixel = info.bytes_per_pixel;

    let offset = (y * info.stride + x).saturating_mul(bytes_per_pixel);

    let Some(pixel_end) = offset.checked_add(bytes_per_pixel) else {
        return;
    };

    if pixel_end > buffer.len() {
        return;
    }

    let pixel = &mut buffer[offset..pixel_end];
    pixel.fill(0);

    let [red, green, blue] = color;

    match info.pixel_format {
        PixelFormat::Rgb if pixel.len() >= 3 => {
            pixel[0] = red;
            pixel[1] = green;
            pixel[2] = blue;
        }

        PixelFormat::Bgr if pixel.len() >= 3 => {
            pixel[0] = blue;
            pixel[1] = green;
            pixel[2] = red;
        }

        PixelFormat::U8 if !pixel.is_empty() => {
            let grayscale = (u16::from(red) + u16::from(green) + u16::from(blue)) / 3;

            pixel[0] = grayscale as u8;
        }

        PixelFormat::Unknown {
            red_position,
            green_position,
            blue_position,
        } => {
            let mut packed = 0_u32;

            if red_position < 32 {
                packed |= u32::from(red) << red_position;
            }

            if green_position < 32 {
                packed |= u32::from(green) << green_position;
            }

            if blue_position < 32 {
                packed |= u32::from(blue) << blue_position;
            }

            for (index, byte) in pixel.iter_mut().take(4).enumerate() {
                *byte = ((packed >> (index * 8)) & 0xff) as u8;
            }
        }

        _ => {}
    }
}

fn serial_port() -> SerialPort {
    let mut port = unsafe { SerialPort::new(0x3f8) };

    port.init();
    port
}

fn halt_loop() -> ! {
    loop {
        hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut serial = serial_port();

    let _ = writeln!(serial);
    let _ = writeln!(serial, "========================================");
    let _ = writeln!(serial, " pvKernel PANIC");
    let _ = writeln!(serial, "{info}");
    let _ = writeln!(serial, "========================================");

    halt_loop()
}
