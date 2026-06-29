use rusb::DeviceHandle;
use std::time::Duration;

const VENDOR_ID: u16 = 0x09CB;
const PRODUCT_ID: u16 = 0x1996;

const TIMEOUT_FRAME: Duration = Duration::from_millis(100);
const TIMEOUT_CTRL: Duration = Duration::from_millis(100);
const TIMEOUT_POLL: Duration = Duration::from_millis(10);

// bmRequestType = 0x01: direction=out, type=standard, recipient=interface
const RT_IFACE_OUT: u8 = 0x01;
// bRequest = SET_INTERFACE (alternate setting)
const REQ_SET_IFACE: u8 = 0x0b;

pub struct Camera {
    handle: DeviceHandle<rusb::GlobalContext>,
}

impl Camera {
    pub fn open() -> Result<Self, String> {
        let handle = rusb::open_device_with_vid_pid(VENDOR_ID, PRODUCT_ID)
            .ok_or_else(|| format!("FLIR One not found (VID:{:#06x} PID:{:#06x})", VENDOR_ID, PRODUCT_ID))?;

        handle
            .set_active_configuration(3)
            .map_err(|e| format!("set_active_configuration: {}", e))?;

        for iface in 0u8..3 {
            handle
                .claim_interface(iface)
                .map_err(|e| format!("claim_interface {}: {}", iface, e))?;
        }

        // Stop any running interfaces before starting fresh
        for w_index in [2u16, 1] {
            handle
                .write_control(RT_IFACE_OUT, REQ_SET_IFACE, 0, w_index, &[], TIMEOUT_CTRL)
                .map_err(|e| format!("stop interface {}: {}", w_index, e))?;
        }

        // Start FILEIO (interface 1) then FRAME (interface 2)
        for w_index in [1u16, 2] {
            handle
                .write_control(RT_IFACE_OUT, REQ_SET_IFACE, 1, w_index, &[], TIMEOUT_CTRL)
                .map_err(|e| format!("start interface {}: {}", w_index, e))?;
        }

        Ok(Camera { handle })
    }

    /// Read from the frame endpoint (0x85) into `buf`.
    /// Returns the number of bytes read, or 0 on timeout.
    pub fn read_ep85(&self, buf: &mut [u8]) -> Result<usize, rusb::Error> {
        match self.handle.read_bulk(0x85, buf, TIMEOUT_FRAME) {
            Ok(n) => Ok(n),
            Err(rusb::Error::Timeout) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Drain the two control/status endpoints so they don't stall.
    pub fn drain_control_eps(&self) {
        let mut tmp = [0u8; 4096];
        let _ = self.handle.read_bulk(0x81, &mut tmp, TIMEOUT_POLL);
        let _ = self.handle.read_bulk(0x83, &mut tmp, TIMEOUT_POLL);
    }
}
