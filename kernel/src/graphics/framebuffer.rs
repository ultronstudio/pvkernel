use bootloader_api::info::{FrameBuffer, FrameBufferInfo, PixelFormat};

pub fn render_boot_screen(framebuffer: &mut FrameBuffer) {
    let info = framebuffer.info();
    let buffer = framebuffer.buffer_mut();

    let background = [8, 12, 20];
    let accent = [0, 183, 239]; // #00B7EF

    // Fill the entire background
    fill_rect(buffer, info, 0, 0, info.width, info.height, background);

    // Logo/P size
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

    // Left vertical stem
    fill_rect(buffer, info, stem_x, top_y, stroke, size * 3 / 4, color);

    // Top horizontal bar
    fill_rect(buffer, info, stem_x, top_y, bowl_w, stroke, color);

    // Middle horizontal bar
    fill_rect(buffer, info, stem_x, top_y + bowl_h, bowl_w, stroke, color);

    // Right vertical part of the upper loop
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
