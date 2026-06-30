mod camera;
mod display;
mod frame;
mod palette;

use std::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn on_signal(_: libc::c_int) {
    RUNNING.store(false, Ordering::Relaxed);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let fb_path = find_arg(&args, "--fb").unwrap_or("/dev/fb0");

    unsafe {
        libc::signal(libc::SIGINT, on_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as libc::sighandler_t);
    }

    eprintln!("flirpi: opening framebuffer {}", fb_path);
    let mut fb = match display::Framebuffer::open(fb_path) {
        Ok(fb) => {
            eprintln!(
                "flirpi: framebuffer {}x{} {}bpp  {}",
                fb.width(), fb.height(), fb.bpp(),
                fb.pixel_format_str()
            );
            fb
        }
        Err(e) => {
            eprintln!("flirpi: cannot open framebuffer: {}", e);
            std::process::exit(1);
        }
    };

    let palette = palette::iron();

    eprintln!("flirpi: waiting for FLIR One (VID:0x09CB PID:0x1996) …");

    // Outer loop: reconnect on camera disconnect
    while RUNNING.load(Ordering::Relaxed) {
        match camera::Camera::open() {
            Err(e) => {
                eprintln!("flirpi: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            Ok(mut cam) => {
                eprintln!("flirpi: camera connected");
                capture_loop(&mut cam, &mut fb, &palette);
                eprintln!("flirpi: camera disconnected");
            }
        }
    }

    eprintln!("flirpi: exiting");
}

fn capture_loop(
    cam: &mut camera::Camera,
    fb: &mut display::Framebuffer,
    palette: &[[u8; 3]; 256],
) {
    let mut accum = frame::FrameAccumulator::new();
    let mut chunk = vec![0u8; 1 << 20]; // 1 MiB read buffer

    while RUNNING.load(Ordering::Relaxed) {
        match cam.read_ep85(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                if let Some(thermal) = accum.push_chunk(&chunk[..n]) {
                    fb.draw_thermal(&thermal, palette);
                }
            }
            Err(e) => {
                eprintln!("flirpi: USB read error: {}", e);
                break;
            }
        }
        cam.drain_control_eps();
    }
}

fn find_arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}
