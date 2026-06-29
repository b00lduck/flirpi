# flirpi

Displays live thermal imagery from a **FLIR One for Android** camera on a
Raspberry Pi 2 via a SPI-connected LCD framebuffer.  Written in Rust.

The camera is accessed directly over USB (libusb), frames are min-max
normalised and colourised with an iron palette, then written to `/dev/fb0`
scaled to the display resolution.

libusb is compiled from source (`rusb` vendored feature), so no ARM libusb
package is needed on the build host or the Pi.

---

## Dependencies

### Arch Linux — build host

```bash
# Rust toolchain
sudo pacman -S rustup
rustup default stable
rustup target add armv7-unknown-linux-gnueabihf

# zig — used as the cross-compiler (no Docker, no ARM sysroot needed)
sudo pacman -S zig

# cargo-zigbuild
cargo install cargo-zigbuild
```

For the native (x86-64 dev) build only, a C compiler is needed to vendor-compile
libusb.  It is already present if you have `base-devel` installed:

```bash
sudo pacman -S base-devel
```

### Raspberry Pi (runtime)

No extra packages needed — libusb is statically linked into the binary.

---

## Building

### Cross-compile for RPi 2 — Arch Linux (recommended)

Uses [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) with
`zig cc` as the cross-compiler.  No Docker, no ARM sysroot, no ARM libusb
package needed.

```bash
make zigbuild
```

The binary lands at `target/armv7-unknown-linux-gnueabihf/release/flirpi`.

### Cross-compile using Docker (`cross` tool)

```bash
# Install the cross tool (one-time, takes a few minutes to compile)
cargo install cross --git https://github.com/cross-rs/cross

# Make sure Docker is running
sudo systemctl start docker

make cross-docker
```

`Cross.toml` selects the `:edge` image (Ubuntu 22.04) to avoid glibc
version mismatches on rolling-release hosts.

### Cross-compile with a native ARM toolchain — Debian/Ubuntu hosts only

```bash
sudo apt install gcc-arm-linux-gnueabihf
make cross
```

### Native build (x86-64, for development)

```bash
make build      # debug
make release    # optimised
```

---

## Deploying to the Pi

```bash
make deploy HOST=pi@192.168.x.x
```

Or manually:

```bash
scp target/armv7-unknown-linux-gnueabihf/release/flirpi pi@192.168.x.x:~/
```

---

## Pi setup

### USB access without root

Create `/etc/udev/rules.d/77-flirone.rules`:

```
SUBSYSTEM=="usb", ATTRS{idVendor}=="09cb", ATTRS{idProduct}=="1996", MODE="0666"
```

Then reload rules:

```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
```

### SPI LCD framebuffer

Enable the fbtft driver for your display in `/boot/config.txt`, for example
for an ILI9341 (320×240) on default SPI pins:

```
dtoverlay=ili9341,speed=32000000,rotate=90
```

After reboot, `/dev/fb1` should appear (fb0 is usually the HDMI output).

---

## Usage

```bash
# Default: reads from /dev/fb0
flirpi

# Specify framebuffer device (e.g. SPI LCD is fb1)
flirpi --fb /dev/fb1
```

The app reconnects automatically if the camera is unplugged and re-attached.
Exit with `Ctrl-C`.

---

## Camera

| Property | Value |
|----------|-------|
| USB VID:PID | `09CB:1996` |
| Thermal sensor | Lepton 3 — 160×120 px, 16-bit |
| Visible camera | JPEG 640×480 (not used here) |
| Frame rate | ~9 fps |
