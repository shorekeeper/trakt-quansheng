//! USART1 command endpoint at 38400 8N1 on PA9 TX and PA10 RX.
//!
//! protocol recognizes screenshot, channel export, and schema-specific import
//! requests. The application validates and confirms import before RAM mutation.
//! Acknowledgement means staged or rejected, never applied or saved.
//!
//! Text diagnostics use fixed 77-byte packets. Binary replies use one static
//! 1044-byte capture buffer with a twenty-byte header. Copying a completed
//! frame or channel bank makes the packet stable while foreground work resumes.
//!
//! RX uses a DMA ring. Each pump consumes at most 64 buffered bytes and advances
//! at most eight TX polls. A blocked TX stage has a twenty-tick deadline;
//! text packets use a 200-tick total and binary packets use 3000 ticks.
//! TC confirms final drain before the buffer can be reused.
//!
//! Capture starts only when its caller admits a stable LCD frame and no
//! engaged transmission. The endpoint does not own RF or journal admission.
//! Transport failure stops USART and DMA; it does not authorize retry or mutate
//! the staged bank through an alternative path.

use core::ptr::addr_of_mut;

use crate::app::time::reached;
use crate::mcu;
use crate::protocol::packets;
use crate::ui::Frame;

const USART: usize = 0x4001_3800;
const BAUD: u32 = 38_400;
pub(crate) use super::serial_view::View as SerialView;

pub(crate) fn diagnostic() -> SerialView {
    super::uart_rx::view()
}

pub(crate) const fn divisor(pclk: u32) -> Option<u32> {
    if pclk < BAUD * 16 || pclk > 72_000_000 { return None; }
    let value = (pclk + BAUD / 2) / BAUD;
    let represented = value * BAUD;
    if value > 65_535 || pclk.abs_diff(represented) > represented / 100 {
        None
    } else {
        Some(value)
    }
}

struct Packet {
    bytes: [u8; 77],
}

impl Packet {
    const fn new(values: [u32; 5]) -> Self {
        let labels: [&[u8]; 5] = [
            b" tick=", b" hclk=", b" pclk=", b" lost=", b" drop=",
        ];
        let hex = b"0123456789ABCDEF";
        let mut bytes = [0u8; 77];
        let mut at = 0;
        while at < 5 {
            bytes[at] = b"TRAKT"[at];
            at += 1;
        }
        let mut field = 0;
        while field < 5 {
            let mut index = 0;
            while index < labels[field].len() {
                bytes[at] = labels[field][index];
                at += 1;
                index += 1;
            }
            index = 0;
            while index < 8 {
                bytes[at] = hex[((values[field] >> ((7 - index) * 4)) & 15) as usize];
                at += 1;
                index += 1;
            }
            field += 1;
        }
        bytes[75] = b'\r';
        bytes[76] = b'\n';
        Self { bytes }
    }
}

#[repr(C)]
struct Capture {
    bytes: [u8; 1044],
}

#[used]
#[no_mangle]
static mut TRAKT_CAPTURE: Capture = Capture { bytes: [0; 1044] };



pub(crate) struct Tx {
    packet: Packet,
    capture: &'static mut Capture,
    capture_mode: bool,
    transfer_length: usize,
    capture_kind: u8,
    export_requested: bool,
    import_announced: bool,
    acknowledgement: u8,
    requested: bool,
    receive: super::uart_rx::Rx,
    captures: u32,
    position: usize,
    active: bool,
    live: bool,
    progress_deadline: u32,
    frame_deadline: u32,
    pub(crate) frames: u32,
    pub(crate) bytes: u32,
    pub(crate) failures: u32,
    pub(crate) dropped: u32,
}

impl Tx {
    /// construct once after BSS initialization. The caller owns
    /// USART1, PA9, PA10, and the static capture buffer.
    pub(crate) unsafe fn initialize(pclk: u32) -> Option<Self> {
        let divider = divisor(pclk)?;
        let receive;

        unsafe {
            mcu::enable_gpio();
            mcu::modify(mcu::RCC + 0x40, 1 << 14, 1 << 14);
            let _ = mcu::read(mcu::RCC + 0x40);
            mcu::modify(mcu::RCC + 0x30, 1 << 14, 1 << 14);
            mcu::modify(mcu::RCC + 0x30, 1 << 14, 0);

            mcu::write(USART + 12, 0);
            mcu::write(USART + 16, 0);
            mcu::write(USART + 20, 0);
            mcu::write(USART + 8, divider);

            mcu::write(mcu::GPIO_A + 0x18, (1 << 9) | (1 << 10));
            mcu::modify(mcu::GPIO_A + 4, (1 << 9) | (1 << 10), 0);
            mcu::modify(mcu::GPIO_A + 8, (3 << 18) | (3 << 20), (1 << 18) | (1 << 20));
            mcu::modify(mcu::GPIO_A + 12, (3 << 18) | (3 << 20), (1 << 18) | (1 << 20));
            mcu::modify(mcu::GPIO_A + 0x24, (15 << 4) | (15 << 8), (1 << 4) | (1 << 8));
            mcu::modify(mcu::GPIO_A, (3 << 18) | (3 << 20), (2 << 18) | (2 << 20));

            receive = super::uart_rx::Rx::initialize();
            mcu::write(USART + 12, (1 << 13) | (1 << 3) | (1 << 2));
        }

        Some(Self {
            packet: Packet::new([0; 5]),
            capture: unsafe { &mut *addr_of_mut!(TRAKT_CAPTURE) },
            capture_mode: false,
            transfer_length: 1044,
            capture_kind: 0,
            export_requested: false,
            import_announced: false,
            acknowledgement: 0,
            requested: false,
            receive,
            captures: 0,
            position: 0,
            active: false,
            live: true,
            progress_deadline: 0,
            frame_deadline: 0,
            frames: 0,
            bytes: 0,
            failures: 0,
            dropped: 0,
        })
    }

    pub(crate) fn queue(&mut self, now: u32, values: [u32; 5]) {
        if !self.live || self.active || self.requested
            || self.export_requested || self.acknowledgement != 0
        {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.packet = Packet::new(values);
        self.capture_mode = false;
        self.position = 0;
        self.progress_deadline = now.wrapping_add(20);
        self.frame_deadline = now.wrapping_add(200);
        self.active = true;
    }

    pub(crate) fn capture_requested(&self) -> bool {
        self.live && self.requested && !self.active
    }

    /// Copy only after the last LCD frame has drained and before drawing
    /// the next one. The caller retains the sole framebuffer ownership.
    #[inline(never)]
    pub(crate) fn capture(&mut self, now: u32, frame: &Frame) {
        if !self.capture_requested() { return; }

        packets::screenshot(&mut self.capture.bytes, now, frame);

        self.requested = false;
        self.capture_mode = true;
        self.transfer_length = 1044;
        self.capture_kind = 0;
        self.position = 0;
        self.progress_deadline = now.wrapping_add(20);
        self.frame_deadline = now.wrapping_add(3_000);
        self.active = true;
    }

    fn receive(&mut self, now: u32) {
        if self.receive.poll(now) && (!self.capture_mode || !self.active) {
            self.requested = true;
        }
        self.export_requested |= self.receive.take_export();
    }

    pub(crate) fn export_requested(&self) -> bool {
        self.live && self.export_requested && !self.active
            && self.acknowledgement == 0
    }

    /// Fill the existing stable capture buffer; no second export-sized
    /// array is allocated. The callback must write exactly sixteen slots.
    #[inline(never)]
    pub(crate) fn export_channels(
        &mut self,
        now: u32,
        fill: impl FnOnce(&mut [u8]),
    ) {
        if !self.export_requested() {
            return;
        }

        packets::channels(&mut self.capture.bytes, now, fill);

        self.export_requested = false;
        self.start_packet(now, 340, 1);
    }

    pub(crate) fn import_version(&self) -> u8 {
        self.receive.import_version()
    }

    pub(crate) fn import_request(&mut self) -> Option<&[u8]> {
        if self.import_announced || !self.receive.import_pending() {
            return None;
        }

        self.import_announced = true;
        self.receive.import_data()
    }

    pub(crate) fn import_data(&self) -> Option<&[u8]> {
        self.receive.import_data()
    }

    /// Status one means validated and staged, not applied or saved.
    /// Status two rejects the application payload without modifying RAM.
    pub(crate) fn acknowledge_import(&mut self, accepted: bool) {
        self.acknowledgement = if accepted { 1 } else { 2 };

        if !accepted {
            self.finish_import();
        }
    }

    pub(crate) fn finish_import(&mut self) {
        self.receive.finish_import();
        self.import_announced = false;
    }

    fn start_packet(&mut self, now: u32, length: usize, kind: u8) {
        self.capture_mode = true;
        self.transfer_length = length;
        self.capture_kind = kind;
        self.position = 0;
        self.progress_deadline = now.wrapping_add(20);
        self.frame_deadline = now.wrapping_add(3000);
        self.active = true;
    }

    fn send_acknowledgement(&mut self, now: u32) {
        if self.active || self.acknowledgement == 0 {
            return;
        }

        packets::acknowledgement(
            &mut self.capture.bytes, now, self.acknowledgement,
        );
        self.acknowledgement = 0;
        self.start_packet(now, 20, 2);
    }

    /// Completed packets, transmitted bytes, failures, and dropped text
    /// requests. An absent endpoint returns four zeroes through this API.
    pub(crate) const fn counters(&self) -> [u32; 4] {
        [self.frames, self.bytes, self.failures, self.dropped]
    }

    pub(crate) fn publish(&self, now: u32) {
        let flags = self.live as u32
            | ((self.active as u32) << 1)
            | ((self.capture_mode as u32) << 2)
            | ((self.requested as u32) << 3);
        self.receive.publish(
            now, self.captures, self.bytes, self.failures, flags,
        );
    }

    pub(crate) fn pump(&mut self, now: u32) {
        if !self.live { return; }
        self.receive(now);
        self.send_acknowledgement(now);
        if !self.active { return; }

        let length = if self.capture_mode { self.transfer_length } else { 77 };
        for _ in 0..8 {
            let status = unsafe { mcu::read(USART) };
            if self.position == length && status & (1 << 6) != 0 {
                self.active = false;
                self.frames = self.frames.saturating_add(1);
                if self.capture_mode && self.capture_kind == 0 {
                    self.captures = self.captures.saturating_add(1);
                }
                return;
            }
            if reached(now, self.frame_deadline) {
                self.fail();
                return;
            }

            if self.position < length && status & (1 << 7) != 0 {
                let value = if self.capture_mode {
                    self.capture.bytes[self.position]
                } else {
                    self.packet.bytes[self.position]
                };
                unsafe { mcu::write(USART + 4, value as u32) };
                self.position += 1;
                self.bytes = self.bytes.saturating_add(1);
                self.progress_deadline = now.wrapping_add(20);
            } else {
                if reached(now, self.progress_deadline) {
                    self.fail();
                }
                return;
            }
        }
    }

    fn fail(&mut self) {
        self.receive.stop();
        unsafe { mcu::write(USART + 12, 0) };
        self.active = false;
        self.live = false;
        self.requested = false;
        self.failures = self.failures.saturating_add(1);
    }
}

const _: () = {
    assert!(core::mem::size_of::<Capture>() == 1044);
    assert!(matches!(divisor(48_000_000), Some(1250)));
    assert!(matches!(divisor(24_000_000), Some(625)));
    assert!(matches!(divisor(72_000_000), Some(1875)));
    assert!(divisor(500_000).is_none());
    assert!(divisor(72_000_001).is_none());

    let packet = Packet::new([0x1234_ABCD, 0, 1, u32::MAX, 0]);
    assert!(packet.bytes[11] == b'1' && packet.bytes[18] == b'D');
    assert!(packet.bytes[75] == b'\r' && packet.bytes[76] == b'\n');

};