TARGET   := armv7-unknown-linux-gnueabihf
CC_CROSS := arm-linux-gnueabihf-gcc
PKG_PATH := /usr/lib/arm-linux-gnueabihf/pkgconfig

.PHONY: build release zigbuild cross cross-docker check clean deploy

build:
	cargo build

release:
	cargo build --release

# Cross-compile using cargo-zigbuild (recommended for Arch Linux — no Docker needed).
# libusb is vendored (compiled from source), so no ARM sysroot packages are required.
# Prerequisites:
#   sudo pacman -S zig
#   cargo install cargo-zigbuild
zigbuild:
	cargo zigbuild --target $(TARGET).2.17 --release

# Cross-compile using the `cross` tool + Docker.
# Install: cargo install cross --git https://github.com/cross-rs/cross
# Requires Docker to be running.
cross-docker:
	cross build --target $(TARGET) --release

# Cross-compile with a native ARM toolchain (Debian/Ubuntu only).
# Prerequisites:
#   sudo apt install gcc-arm-linux-gnueabihf
cross:
	cargo build --target $(TARGET) --release

check:
	cargo check

clean:
	cargo clean

# Deploy to a running Pi over SSH.
# Usage: make deploy HOST=pi@192.168.x.x
# Uses whichever binary was built last (cross or cross-docker).
deploy:
	scp target/$(TARGET)/release/flirpi $(HOST):~/flirpi || \
		(ssh $(HOST) 'killall flirpi'; scp target/$(TARGET)/release/flirpi $(HOST):~/flirpi)
	@echo "Deployed to $(HOST)"
