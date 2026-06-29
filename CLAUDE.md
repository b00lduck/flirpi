# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build commands

```bash
make          # debug build (native)
make release  # release build (native)
make cross    # cross-compile for RPi 2 (armv7-unknown-linux-gnueabihf)
make check    # type-check without linking
cargo test    # (no tests yet)

# Deploy binary to a running Pi
make deploy HOST=pi@192.168.x.x
```

## Cross-compilation

`rusb` is built with `features = ["vendored"]`, which compiles libusb from source via the `cc` crate.  No ARM libusb package is needed on any build host, and the resulting binary statically links libusb (no runtime dep on the Pi either).

**Option A — cargo-zigbuild (recommended for Arch Linux):**

```bash
sudo pacman -S zig
cargo install cargo-zigbuild
make zigbuild
```

No Docker needed. `zig cc` acts as the cross-compiler; the `.2.17` glibc suffix ensures the binary runs on older Raspberry Pi OS releases.

**Option B — Docker via `cross`:**

```bash
cargo install cross --git https://github.com/cross-rs/cross
sudo systemctl start docker
make cross-docker
```

`Cross.toml` selects the `:edge` image (Ubuntu 22.04) to avoid glibc version mismatches on rolling-release hosts.

**Option C — native ARM toolchain (Debian/Ubuntu only):**

```bash
rustup target add armv7-unknown-linux-gnueabihf
sudo apt install gcc-arm-linux-gnueabihf
make cross
```

## Architecture

The app has four modules:

| Module | Responsibility |
|--------|----------------|
| `camera` | libusb communication with the FLIR One (via `rusb`) |
| `frame` | Rolling 1 MiB buffer; accumulates USB chunks → complete frames; thermal pixel extraction and min-max normalisation |
| `palette` | Iron thermal colour palette (256 × RGB) computed at startup |
| `display` | Linux framebuffer via `libc` ioctls + mmap; nearest-neighbour scale to fill any display |

`main` owns the outer reconnect loop and the inner capture loop.  The camera, framebuffer, and accumulator are all single-threaded.

## FLIR One USB protocol

- **USB identifiers**: VID `0x09CB`, PID `0x1996`, configuration 3, interfaces 0–2.
- **Stream start**: two `SET_INTERFACE` control transfers (`bmRequestType=0x01`, `bRequest=0x0b`) — stop both interfaces, then start FILEIO (1) then FRAME (2).
- **Frame data**: bulk read from endpoint `0x85` with a 100 ms timeout.
- **Control endpoints** `0x81` and `0x83` are drained each iteration to prevent stalls.

## Frame format

```
Offset  Bytes  Field
0       4      Magic: EF BE 00 00
8       4      FrameSize   (u32 LE) — payload size after byte 27
12      4      ThermalSize (u32 LE)
16      4      JpgSize     (u32 LE)
20      4      StatusSize  (u32 LE)
28      …      Thermal data
28+T    …      JPEG (visible camera)
28+T+J  …      Status JSON
```

**Thermal pixel extraction** — 160×120 sensor, row stride 164 px (two 2-px gaps per row):

```
x < 80:  buf[32 + 2*(y*164 + x)]
x ≥ 80:  buf[36 + 2*(y*164 + x)]   (+4 skips the mid-row gap)
```

Values are 16-bit unsigned; `frame.rs` normalises them to 8-bit with min-max scaling.

**FFC suppression**: if the string `"FFC"` appears at `28 + ThermalSize + JpgSize + 17` in the status block, the shutter is doing a flat-field correction.  That frame *and the next one* are dropped to avoid a bright-flash artefact.

## Framebuffer

`display.rs` reads `FBIOGET_VSCREENINFO` / `FBIOGET_FSCREENINFO` at open time and uses the reported `red/green/blue.offset` and `.length` fields to encode pixels.  This handles RGB565, BGR565 (common fbtft SPI LCD drivers), and 32-bit formats without any hardcoded format assumptions.

Pass a different device with `--fb /dev/fb1`.
