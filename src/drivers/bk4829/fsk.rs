//! BK4829 FIFO modem under the existing foreground RX or TX owner.
//!
//! The register profile follows armel/uv-k1-k5v3-firmware-custom,
//! App/driver/bk4829.c SetupAircopy, SendFSKData, and PrepareFSKReceive.
//! Gogu-Qs/GOGUFW-UV-K1-Messenger App/app/messenger_rf.c provides a
//! BK4829 reference for event-driven FIFO service and long TX preamble.
//! Register values are hardware reference data, not imported application code.
//!
//! REG_58=00C1 selects the reference nominal 1200-bit/s FSK profile;
//! REG_72=3065 and REG_70=00C3 retain its oscillator and amplitude codes.
//! REG_5C=5665 retains the reference hardware CRC configuration. Software
//! packet admission remains independent. TKF1 statuses use crate::fsk;
//! TKL1 messages use crate::link. Both validate their complete software CRC,
//! reserved fields, and framing. REG_5D=4700 selects 72 bytes.
//! REG_5E=3204 is retained with four-word RX FIFO service.
//!
//! REG_59 uses 4068 then 3068 for RX clear/enable with scrambling.
//! TX uses 80F8 then 00F8 for FIFO clear/release, and 28F8 to start
//! scrambled transmission with a sixteen-byte preamble and four-byte sync.
//! Sync registers are explicitly programmed to 85CF and AB45.
//!
//! Neither owner writes frequency, bandwidth, PA, LNA, RF-link, or amplifier
//! controls. RX requires a settled FM receiver with signaling disabled.
//! TX requires an already admitted transmitter with MIC ADC disabled.
//! The transmitter retains its original policy, calibration, battery,
//! cancellation, foreground deadline, and independent SysTick backstop.
//!
//! No MCU interrupt is enabled. Each poll consumes at most one event set:
//! read REG_0C, write zero to REG_02, then read REG_02. TX completion requires
//! bit 15 of the consumed events, not merely the common pending indicator.
//! RX uses sync bit one, FIFO bit twelve, and completion bit thirteen.
//!
//! Cleanup disables the modem and both generators before restoring the six
//! passive configuration registers not rebuilt by normal RX initialization.
//! It never restores an enabled event mask or a live RX/TX command.
//! The RF owner removes PA before TX cleanup and uses normal RX restoration.
//!
//! RX partial collection has a fixed deadline. A foreground service gap
//! above 100 observed ticks discards and clears the FIFO rather than claiming
//! continuity. These timers do not measure periods lost under PRIMASK.
//! Hardware behavior and modulation level require separate radio testing.

use super::{read, write};
use crate::app::time::reached;
use crate::fsk::{self, Collector};

const PASSIVE: [u8; 6] = [0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x72];
const RX_MASK: u16 = 0x3002;
const TX_MASK: u16 = 0x8000;

const PROFILE: [(u8, u16); 8] = [
    (0x72, 0x3065),
    (0x70, 0x00C3),
    (0x58, 0x00C1),
    (0x5C, 0x5665),
    (0x5D, 0x4700),
    (0x5E, 0x3204),
    (0x5A, 0x85CF),
    (0x5B, 0xAB45),
];

fn save() -> [u16; 6] {
    let mut saved = [0u16; 6];
    for index in 0..saved.len() {
        saved[index] = read(PASSIVE[index]);
    }
    saved
}

fn restore(saved: [u16; 6]) {
    write(0x59, 0);
    write(0x3F, 0);
    write(0x70, 0);
    write(0x58, 0);

    for index in 0..saved.len() {
        write(PASSIVE[index], saved[index]);
    }
}

fn acknowledge() -> u16 {
    write(0x02, 0);
    read(0x02)
}

fn events() -> u16 {
    if read(0x0C) & 1 == 0 { 0 } else { acknowledge() }
}

fn check_profile(mask: u16) -> Result<(), u32> {
    for &(register, expected) in &PROFILE {
        if read(register) != expected {
            return Err(0x7900 + register as u32);
        }
    }

    if read(0x3F) != mask {
        return Err(0x793F);
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) armed: bool,
    pub(crate) accepted: u32,
    pub(crate) rejected: u32,
    pub(crate) discarded: u32,
    pub(crate) last_sequence: u16,
    pub(crate) last_status: u8,
    pub(crate) last_events: u16,
    pub(crate) pending_words: usize,
}

impl View {
    pub(crate) const fn empty() -> Self {
        Self {
            armed: false,
            accepted: 0,
            rejected: 0,
            discarded: 0,
            last_sequence: 0,
            last_status: 0,
            last_events: 0,
            pending_words: 0,
        }
    }
}

pub(crate) struct Rx {
    pending_message: Option<crate::link::Packet>,
    collector: Collector,
    saved: [u16; 6],
    armed: bool,
    last_poll: u32,
    accepted: u32,
    rejected: u32,
    discarded: u32,
    last_sequence: u16,
    last_status: u8,
    last_events: u16,
}

impl Rx {
    pub(crate) const fn new() -> Self {
        Self {
            pending_message: None,
            collector: Collector::new(),
            saved: [0; 6],
            armed: false,
            last_poll: 0,
            accepted: 0,
            rejected: 0,
            discarded: 0,
            last_sequence: 0,
            last_status: 0,
            last_events: 0,
        }
    }

    pub(crate) const fn armed(&self) -> bool {
        self.armed
    }

    /// One complete checked Link packet between RF service and application
    /// observation. The foreground consumes it after each receiver pump.
    /// TKF1 status packets retain their independent counters and LAST fields.
    pub(crate) fn take_packet(&mut self) -> Option<crate::link::Packet> {
        self.pending_message.take()
    }

    pub(crate) fn view(&self) -> View {
        View {
            armed: self.armed,
            accepted: self.accepted,
            rejected: self.rejected,
            discarded: self.discarded,
            last_sequence: self.last_sequence,
            last_status: self.last_status,
            last_events: self.last_events,
            pending_words: self.collector.count(),
        }
    }

    fn clear_fifo(&mut self) {
        self.collector.discard();
        write(0x59, 0x4068);
        write(0x59, 0x3068);
    }

    /// Requires exclusive settled RX ownership and no tone detector.
    /// Audio and RF links retain the receiver's normal checked controls.
    fn start(&mut self, now: u32) -> Result<(), u32> {
        if read(0x70) != 0 || read(0x58) & 1 != 0
            || read(0x59) & 0x1800 != 0 || read(0x3F) != 0
        {
            return Err(0x7802);
        }

        self.saved = save();
        self.armed = true;
        self.last_poll = now;
        self.collector.discard();

        write(0x59, 0);
        write(0x3F, 0);
        for &(register, value) in &PROFILE {
            write(register, value);
        }
        let _ = acknowledge();
        write(0x3F, RX_MASK);
        self.clear_fifo();
        self.check()
    }

    pub(crate) fn check(&self) -> Result<(), u32> {
        if self.armed { check_profile(RX_MASK) } else { Ok(()) }
    }

    /// Stop before RF retuning, tone programming, local audio, or TX.
    /// Observed counters and the last accepted status remain available.
    pub(crate) fn stop(&mut self) {
        if !self.armed {
            return;
        }

        restore(self.saved);
        self.armed = false;
        self.collector.discard();
    }

    #[inline(never)]
    pub(crate) fn pump(&mut self, now: u32, enabled: bool) -> Result<(), u32> {
        if !enabled {
            self.stop();
            return Ok(());
        }
        if !self.armed {
            return self.start(now);
        }

        if now.wrapping_sub(self.last_poll) > 100 {
            self.discarded = self.discarded.saturating_add(1);
            let _ = acknowledge();
            self.clear_fifo();
            self.last_poll = now;
            return Ok(());
        }
        self.last_poll = now;

        if self.collector.expire(now) {
            self.discarded = self.discarded.saturating_add(1);
            let _ = acknowledge();
            self.clear_fifo();
            return Ok(());
        }

        let flags = events();
        if flags == 0 {
            return Ok(());
        }
        self.last_events = flags;

        if flags & 0x0002 != 0 {
            self.collector.sync(now);
        }

        // Completion takes precedence over the threshold event. A final
        // event set can contain both; the remaining words are read once.
        if flags & 0x2000 != 0 {
            let remaining = fsk::WORDS - self.collector.count();
            for _ in 0..remaining {
                let word = read(0x5F);
                if !self.collector.push(now, word) {
                    self.rejected = self.rejected.saturating_add(1);
                    self.clear_fifo();
                    return Ok(());
                }
            }

            if let Some(words) = self.collector.finish_words() {
                if let Some(packet) = fsk::decode(&words) {
                    // Preserve the existing status counter and LAST fields.
                    self.accepted = self.accepted.saturating_add(1);
                    self.last_sequence = packet.sequence;
                    self.last_status = packet.status;
                } else {
                    match crate::link::decode(&words) {
                        Some(packet) => {
                            // Application admission and any response are
                            // separate from hardware packet collection.
                            self.pending_message = Some(packet);
                        }
                        None => {
                            self.rejected = self.rejected.saturating_add(1);
                        }
                    }
                }
            } else {
                self.rejected = self.rejected.saturating_add(1);
            }

            self.clear_fifo();
        } else if flags & 0x1000 != 0 {
            for _ in 0..4 {
                let word = read(0x5F);
                if !self.collector.push(now, word) {
                    self.rejected = self.rejected.saturating_add(1);
                    self.clear_fifo();
                    break;
                }
            }
        }

        Ok(())
    }
}

#[derive(Clone, Copy)]
enum TxPhase {
    Setup,
    Load,
    Wait,
    Running,
}

pub(crate) struct Tx {
    words: [u16; fsk::WORDS],
    saved: [u16; 6],
    phase: TxPhase,
    index: usize,
    wake_at: u32,
    deadline: u32,
    next_check: u32,
}

impl Tx {
    /// The enclosing transmitter already admitted PA and disabled MIC ADC.
    /// No new policy decision or RF-link write is made by this constructor.
    /// A start failure is handled by the enclosing RF-off cleanup.
    pub(crate) fn start(now: u32, status: u8, sequence: u16) -> Self {
        Self::start_words(now, fsk::encode(status, sequence))
    }

    /// Only the FIFO payload differs from a status transmission. Message
    /// construction already established all fields; the same register
    /// sequence, preamble, packet length, checks, and deadline remain.
    pub(crate) fn start_message(now: u32, message: crate::link::Message) -> Self {
        let words = match crate::link::encode(crate::link::Packet::Text(message)) {
            Some(words) => words,
            None => panic!("owned Link message rejected"),
        };
        Self::start_words(now, words)
    }

    /// The transmitter already owns admitted RF with MIC ADC disabled.
    /// An Ack uses exactly the same 72-byte FIFO and register sequence.
    pub(crate) fn start_ack(now: u32, packet: crate::link::Packet) -> Self {
        assert!(matches!(packet, crate::link::Packet::Ack { .. }));
        let words = match crate::link::encode(packet) {
            Some(words) => words,
            None => panic!("owned Link acknowledgement rejected"),
        };
        Self::start_words(now, words)
    }

    fn start_words(now: u32, words: [u16; fsk::WORDS]) -> Self {
        let owner = Self {
            words,
            saved: save(),
            phase: TxPhase::Setup,
            index: 0,
            wake_at: 0,
            deadline: now.wrapping_add(1200),
            next_check: 0,
        };

        write(0x50, 0xBB18);
        write(0x3F, 0);
        write(0x59, 0);
        write(0x70, 0);
        write(0x58, 0);
        let _ = acknowledge();
        owner
    }

    /// Called only after the enclosing owner removes PA and RF links.
    /// Passive settings return to their captured values; no live modem
    /// command, event mask, generator, or audio unmute is restored.
    pub(crate) fn cancel(&mut self) {
        write(0x50, 0xBB18);
        restore(self.saved);
    }

    /// Setup writes one profile register per pass; Load writes one FIFO
    /// word per pass. Running consumes at most one hardware event set.
    /// True means local TX completion only, never remote delivery.
    #[inline(never)]
    pub(crate) fn pump(&mut self, now: u32) -> Result<bool, u32> {
        if reached(now, self.deadline) {
            return Err(0x7803);
        }

        match self.phase {
            TxPhase::Setup => {
                if self.index < PROFILE.len() {
                    let (register, value) = PROFILE[self.index];
                    write(register, value);
                    self.index += 1;
                } else {
                    write(0x3F, TX_MASK);
                    write(0x59, 0x80F8);
                    write(0x59, 0x00F8);
                    self.index = 0;
                    self.phase = TxPhase::Load;
                }
            }
            TxPhase::Load => {
                write(0x5F, self.words[self.index]);
                self.index += 1;

                if self.index == self.words.len() {
                    self.wake_at = now.wrapping_add(20);
                    self.phase = TxPhase::Wait;
                }
            }
            TxPhase::Wait => {
                if reached(now, self.wake_at) {
                    check_profile(TX_MASK)?;
                    write(0x50, 0x3B18);
                    write(0x59, 0x28F8);
                    self.next_check = now.wrapping_add(100);
                    self.phase = TxPhase::Running;
                }
            }
            TxPhase::Running => {
                if events() & TX_MASK != 0 {
                    return Ok(true);
                }

                if reached(now, self.next_check) {
                    check_profile(TX_MASK)?;
                    self.next_check = now.wrapping_add(100);
                }
            }
        }

        Ok(false)
    }
}