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

    /// Write a thermal frame to the framebuffer, scaling to fill the screen.
    pub fn draw_thermal(&mut self, frame: &ThermalFrame, palette: &[[u8; 3]; 256]) {
        let fb_w = self.vinfo.xres as usize;
        let fb_h = self.vinfo.yres as usize;
        let bpp = self.vinfo.bits_per_pixel as usize;
        let bytes_pp = bpp / 8;
        let stride = self.line_length as usize;

        let fb = unsafe { std::slice::from_raw_parts_mut(self.ptr, self.map_size) };

        for dy in 0..fb_h {
            let sy = dy * THERMAL_H / fb_h;
            let row_off = dy * stride;

            for dx in 0..fb_w {
                let sx = dx * THERMAL_W / fb_w;
                let [r, g, b] = palette[frame.gray[sy * THERMAL_W + sx] as usize];

                let off = row_off + dx * bytes_pp;

                match bpp {
                    16 => {
                        // Encode using the hardware-reported channel layout (handles both
                        // RGB565 and BGR565 drivers).
                        let pixel = encode16(r, g, b, &self.vinfo);
                        // Use native byte order; fbtft drivers expect host-endian values.
                        let bytes = pixel.to_ne_bytes();
                        fb[off] = bytes[0];
                        fb[off + 1] = bytes[1];
                    }
                    32 => {
                        let pixel = encode32(r, g, b, &self.vinfo);
                        let bytes = pixel.to_ne_bytes();
                        fb[off..off + 4].copy_from_slice(&bytes);
                    }
                    24 => {
                        // 24-bit packed; kernel reports offsets in bits.
                        write_channel(fb, off, r, &self.vinfo.red);
                        write_channel(fb, off, g, &self.vinfo.green);
                        write_channel(fb, off, b, &self.vinfo.blue);
                    }
                    _ => {}
                }
            }
        }
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

fn encode16(r: u8, g: u8, b: u8, vi: &FbVarScreeninfo) -> u16 {
    let r = (r as u16) >> (8 - vi.red.length);
    let g = (g as u16) >> (8 - vi.green.length);
    let b = (b as u16) >> (8 - vi.blue.length);
    (r << vi.red.offset) | (g << vi.green.offset) | (b << vi.blue.offset)
}

fn encode32(r: u8, g: u8, b: u8, vi: &FbVarScreeninfo) -> u32 {
    ((r as u32) << vi.red.offset)
        | ((g as u32) << vi.green.offset)
        | ((b as u32) << vi.blue.offset)
}

fn write_channel(fb: &mut [u8], pixel_off: usize, val: u8, bf: &FbBitfield) {
    // bf.offset is in bits from the LSB of the pixel's first byte
    let byte_off = pixel_off + (bf.offset / 8) as usize;
    fb[byte_off] = val;
}
