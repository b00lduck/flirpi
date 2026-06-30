// Framebuffer display via Linux /dev/fbX.
//
// Queries FBIOGET_VSCREENINFO and FBIOGET_FSCREENINFO at open time to learn
// the display resolution, pixel format, and line stride.  Supports 16-bit and
// 32-bit pixel formats; the colour-channel layout (RGB/BGR offsets) is read
// from the hardware info so it works with both RGB565 and BGR565 SPI LCD
// drivers (fbtft, etc.).
//
// The 160×120 thermal image is scaled to fill the framebuffer using
// nearest-neighbour interpolation.

use libc::{c_int, c_ulong, c_void, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::ffi::CString;
use std::fmt;

use crate::frame::{ThermalFrame, THERMAL_H, THERMAL_W};

// ── Linux framebuffer ioctl numbers ──────────────────────────────────────────
const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: c_ulong = 0x4602;

// ── Kernel struct mirrors (must match linux/fb.h exactly with #[repr(C)]) ───

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    fb_type: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    // 2 bytes of padding inserted by #[repr(C)] to align line_length to 4 bytes
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

// ── Public API ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct FbError(String);

impl fmt::Display for FbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FbError {}

pub struct Framebuffer {
    fd: c_int,
    ptr: *mut u8,
    map_size: usize,
    vinfo: FbVarScreeninfo,
    line_length: u32,
}

impl Framebuffer {
    pub fn open(path: &str) -> Result<Self, FbError> {
        let cpath = CString::new(path).map_err(|_| FbError("invalid path".into()))?;

        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            return Err(FbError(format!(
                "open {}: {}",
                path,
                std::io::Error::last_os_error()
            )));
        }

        let mut vinfo = FbVarScreeninfo::default();
        let mut finfo = FbFixScreeninfo::default();

        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut vinfo as *mut _) < 0 {
                libc::close(fd);
                return Err(FbError(format!(
                    "FBIOGET_VSCREENINFO: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut finfo as *mut _) < 0 {
                libc::close(fd);
                return Err(FbError(format!(
                    "FBIOGET_FSCREENINFO: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }

        let map_size = (vinfo.yres * finfo.line_length) as usize;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut::<c_void>(),
                map_size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == MAP_FAILED {
            unsafe { libc::close(fd) };
            return Err(FbError(format!(
                "mmap: {}",
                std::io::Error::last_os_error()
            )));
        }

        Ok(Framebuffer {
            fd,
            ptr: ptr as *mut u8,
            map_size,
            vinfo,
            line_length: finfo.line_length,
        })
    }

    pub fn width(&self) -> u32 {
        self.vinfo.xres
    }
    pub fn height(&self) -> u32 {
        self.vinfo.yres
    }
    pub fn bpp(&self) -> u32 {
        self.vinfo.bits_per_pixel
    }

    pub fn pixel_format_str(&self) -> String {
        let vi = &self.vinfo;
        format!(
            "R[off={} len={}] G[off={} len={}] B[off={} len={}]",
            vi.red.offset,
            vi.red.length,
            vi.green.offset,
            vi.green.length,
            vi.blue.offset,
            vi.blue.length,
        )
    }

    /// Write a thermal frame to the framebuffer, scaling to fill the screen.
    pub fn draw_thermal(&mut self, frame: &ThermalFrame, palette: &[[u8; 3]; 256]) {
        let fb_w = self.vinfo.xres as usize;
        let fb_h = self.vinfo.yres as usize;
        let bpp = self.vinfo.bits_per_pixel as usize;
        let bytes_pp = bpp / 8;
        let stride = self.line_length as usize;

        let fb = unsafe { std::slice::from_raw_parts_mut(self.ptr, self.map_size) };

        let tw = THERMAL_W as f32;
        let th = THERMAL_H as f32;

        for dy in 0..fb_h {
            // Map output pixel centre into source space.
            let sy_f = (dy as f32 + 0.5) * th / fb_h as f32 - 0.5;
            let sy0 = (sy_f as isize).clamp(0, THERMAL_H as isize - 1) as usize;
            let sy1 = (sy0 + 1).min(THERMAL_H - 1);
            let ty = sy_f - sy0 as f32;
            let row_off = dy * stride;

            for dx in 0..fb_w {
                let sx_f = (dx as f32 + 0.5) * tw / fb_w as f32 - 0.5;
                let sx0 = (sx_f as isize).clamp(0, THERMAL_W as isize - 1) as usize;
                let sx1 = (sx0 + 1).min(THERMAL_W - 1);
                let tx = sx_f - sx0 as f32;

                // Bilinear interpolation in grayscale space, then palette lookup.
                let g00 = frame.gray[sy0 * THERMAL_W + sx0] as f32;
                let g10 = frame.gray[sy0 * THERMAL_W + sx1] as f32;
                let g01 = frame.gray[sy1 * THERMAL_W + sx0] as f32;
                let g11 = frame.gray[sy1 * THERMAL_W + sx1] as f32;
                let gray = (g00 * (1.0 - tx) * (1.0 - ty)
                    + g10 * tx * (1.0 - ty)
                    + g01 * (1.0 - tx) * ty
                    + g11 * tx * ty
                    + 0.5) as u8;

                let [r, g, b] = palette[gray as usize];

                let off = row_off + dx * bytes_pp;

                match bpp {
                    16 => {
                        // Encode using the hardware-reported channel layout (handles both
                        // RGB565 and BGR565 drivers).
                        let pixel = encode565(r, g, b);
                        // Use native byte order; fbtft drivers expect host-endian values.
                        let bytes = pixel.to_le_bytes();
                        fb[off] = bytes[0];
                        fb[off + 1] = bytes[1];
                    }
                    _ => {}
                }
            }
        }

        // Crosshair overlay
        let cx = fb_w / 2;
        let cy = fb_h / 2;
        const GAP: usize = 5;
        const ARM: usize = 14;
        for dx in cx.saturating_sub(ARM + GAP)..=((cx + ARM + GAP).min(fb_w - 1)) {
            if dx < cx.saturating_sub(GAP) || dx > cx + GAP {
                put16(fb, cy * stride + dx * bytes_pp, 0xFF, 0xFF, 0xFF);
            }
        }
        for dy in cy.saturating_sub(ARM + GAP)..=((cy + ARM + GAP).min(fb_h - 1)) {
            if dy < cy.saturating_sub(GAP) || dy > cy + GAP {
                put16(fb, dy * stride + cx * bytes_pp, 0xFF, 0xFF, 0xFF);
            }
        }

        // Temperature annotation — rotated -90° (reads bottom-to-top), right of crosshair.
        let temp_c = frame.center_raw as f32 * 0.01 - 273.15;
        let label = format!("{:.1}C", temp_c);
        let scale = (fb_h / 120).max(1);
        let label_h = label.chars().count() * (5 * scale + 1);
        let tx = (cx + GAP + 3).min(fb_w.saturating_sub(7 * scale));
        let ty = cy.saturating_sub(label_h / 2);
        draw_text_rot90ccw(fb, stride, bytes_pp, fb_w, fb_h, tx, ty, &label, scale);
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut c_void, self.map_size);
            libc::close(self.fd);
        }
    }
}

// Safety: Framebuffer owns its mmap'd memory exclusively.
unsafe impl Send for Framebuffer {}

// ── Pixel encoding helpers ───────────────────────────────────────────────────

fn encode565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

#[inline]
fn put16(fb: &mut [u8], off: usize, r: u8, g: u8, b: u8) {
    if off + 1 < fb.len() {
        let p = encode565(r, g, b).to_le_bytes();
        fb[off] = p[0];
        fb[off + 1] = p[1];
    }
}

// 5×7 bitmap font — each entry is 7 rows; bit 4 = leftmost column.
fn char_bitmap(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b01110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        _ => return None,
    })
}

// Draw a single glyph rotated -90° (CCW) at screen position (ox, oy).
// Rotated glyph is 7*scale wide × 5*scale tall.
// Two passes: black outline first, white fill second — readable on any background.
fn draw_glyph_rot90ccw(
    fb: &mut [u8],
    stride: usize,
    bytes_pp: usize,
    fb_w: usize,
    fb_h: usize,
    ox: usize,
    oy: usize,
    bm: [u8; 7],
    scale: usize,
) {
    for pass in 0..2u8 {
        for row in 0..7usize {
            for col in 0..5usize {
                if bm[row] & (0x10 >> col) == 0 { continue; }
                // +90°: top row → left side, left col → bottom
                for sy in 0..scale {
                    let py = oy + (4 - col) * scale + sy;
                    for sx in 0..scale {
                        let px = ox + row * scale + sx;
                        if pass == 0 {
                            for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1i32), (0, 1)] {
                                let qx = px as i32 + dx;
                                let qy = py as i32 + dy;
                                if qx >= 0 && (qx as usize) < fb_w && qy >= 0 && (qy as usize) < fb_h {
                                    put16(fb, qy as usize * stride + qx as usize * bytes_pp, 0, 0, 0);
                                }
                            }
                        } else if px < fb_w && py < fb_h {
                            put16(fb, py * stride + px * bytes_pp, 0xFF, 0xFF, 0xFF);
                        }
                    }
                }
            }
        }
    }
}

// Draw a string rotated -90°, characters stacked bottom-to-top, anchored at (ox, oy) top-left.
fn draw_text_rot90ccw(
    fb: &mut [u8],
    stride: usize,
    bytes_pp: usize,
    fb_w: usize,
    fb_h: usize,
    ox: usize,
    oy: usize,
    text: &str,
    scale: usize,
) {
    let char_h = 5 * scale + 1; // rotated char height + 1px gap
    let n = text.chars().count();
    for (i, c) in text.chars().enumerate() {
        let Some(bm) = char_bitmap(c) else { continue };
        // +90° CW reads bottom-to-top → first char at bottom (largest y)
        let cy = oy + (n - 1 - i) * char_h;
        draw_glyph_rot90ccw(fb, stride, bytes_pp, fb_w, fb_h, ox, cy, bm, scale);
    }
}
