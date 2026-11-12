//! Board-specific FM/AM reception, carrier SQL, and signaling audio admission.
//!
//! RX admits 50..<630 MHz and 840..1300 MHz. Both upper intervals use the
//! board path selected above 280 MHz. Register programming does not establish
//! sensitivity or frequency lock. Retuning mutes the
//! amplifier, disables RF links, releases both LNA outputs, writes frequency,
//! bandwidth, correction, and signaling, then selects exactly one RX path.
//! A fifty-tick settling interval follows; no frequency-lock bit is assumed.
//!
//! Initialization applies one preset per foreground pass. Frequency words use
//! ten-hertz units. Volume and DAC codes come from validated calibration;
//! REG_48 adds no Gain-1 attenuation. This output setting does not modify
//! microphone gain or transmit modulation.
//!
//! RSSI is sampled every twenty ticks, or every five ticks in CW. Noise and
//! glitch retain their twenty-tick cadence. CW supplies a separate qualified
//! RSSI envelope to its decoder and optional speaker gate. The ordinary
//! carrier SQL remains independent and retains its admission role.
//! FM carrier SQL uses the complete factory threshold row. CTCSS/DCS
//! has an independent gate: a mismatch can mute audio without hiding carrier.
//! AM uses its separate software RSSI scale and ignores saved CTCSS/DCS.
//! The request's modulation selects thresholds before this owner is pumped.
//! Noise and glitch remain raw observations, not AM squelch conditions.
//! Mode changes retune with fresh histories even when frequency is unchanged.
//! Monitor bypasses both audio conditions; SQL zero bypasses carrier SQL only.
//!
//! PTT/Spectrum suppress speaker audio. During selected-VFO PTT qualification,
//! keep_carrier lets SQL continue under that mute. No receive event, tone
//! match, or audio bypass can authorize transmission.
//!
//! The receiver alone owns its RF transitions while pumped. Readback failure
//! parks the chip and requires an explicit off/on request. PA control remains
//! zero. Register retention is checked, not inferred as proof of RF performance.



use super::{inhibit, read, verify_no_tx, write};
use crate::app::squelch::{Gate, Metrics};
use crate::app::time::reached;
use crate::storage::squelch::Thresholds;
use crate::mcu;
use crate::storage::{calibration::Summary, View as MemoryView};

pub(crate) use crate::profile::Band;

impl Band {
    // Physical LNA selection remains local to the board-specific driver.
    const fn gpio(self) -> u16 {
        match self {
            Self::Vhf => 0x9044,
            Self::Uhf => 0x9048,
        }
    }
}

/// Board-specific analog presets from the BK4829 reference port.
/// FM demodulation values are not interchangeable with the BK4819 profile.
/// Tone generation, DTMF, VOX, companding, and RF events start disabled.
/// The tune path subsequently enables only requested CTCSS detector events.
const PRESETS: [(u8, u16); 34] = [
    (0x10, 0x0318), (0x11, 0x033A), (0x12, 0x03DB),
    (0x13, 0x03DF), (0x14, 0x0210), (0x49, 0x2AB2),
    (0x7B, 0x73DC), (0x7D, 0xE920), (0x40, 0x3516),
    (0x1C, 0x07C0), (0x1D, 0xE555), (0x1E, 0x4C58),
    (0x1F, 0xC65A), (0x3E, 0x94C6), (0x73, 0x4691),
    (0x77, 0x88EF), (0x19, 0x1041), (0x28, 0x0B40),
    (0x29, 0xAA00), (0x2C, 0x1822), (0x2F, 0x9890),
    (0x53, 0x2028), (0x7E, 0x302E), (0x46, 0x600A),
    (0x4A, 0x5430), (0x42, 0x6B5A), (0x2A, 0x7400),
    (0x2B, 0x0000), (0x54, 0x9009), (0x55, 0x31A9),
    (0x70, 0x0000), (0x24, 0x0000), (0x51, 0x0000),
    (0x3F, 0x0000),
];

/// REG_48 follows the BK4829 reference port's field layout:
/// bits 11:10 select additional attenuation, bits 9:4 contain the six-bit
/// volume code, and bits 3:0 contain the four-bit DAC code. The established
/// upper nibble is retained. Gain-1 is zero, with no added attenuation.
/// Local tone playback owns its separate gain setting and restores this
/// register before the ordinary receive tune path resumes.
const fn audio_gain(volume: u8, dac: u8) -> u16 {
    assert!(volume < 64 && dac < 16);
    0xB000 | ((volume as u16) << 4) | dac as u16
}

const fn admitted_write(register: u8, value: u16) -> bool {
    match register {
        0x30 => value == 0 || value == 0xBFF1,
        0x33 => super::rx_gpio_allowed(value),
        0x36 => value == 0,
        0x37 => value == 0x9D1F || value == 0x9F1F,
        0x47 => super::rx_af_allowed(value),
        _ => true,
    }
}

fn configure(register: u8, value: u16) -> Result<(), u32> {
    if !admitted_write(register, value) {
        return Err(0x3300 + register as u32);
    }
    write(register, value);
    Ok(())
}

fn amplifier(on: bool) {
    unsafe {
        mcu::write(mcu::GPIO_A + 0x18, if on { 1 << 8 } else { 1 << 24 });
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Off,
    Wait,
    Program,
    Settle,
    Receive,
    Failed,
}

impl Phase {
    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::Off => b"OFF",
            Self::Wait => b"WAIT CAL",
            Self::Program => b"PROGRAM",
            Self::Settle => b"SETTLE",
            Self::Receive => b"RECEIVE",
            Self::Failed => b"FAULT",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Request {
    pub(crate) enabled: bool,
    pub(crate) frequency: u32,
    pub(crate) narrow: bool,
    pub(crate) am: bool,
    pub(crate) monitor: bool,
    pub(crate) squelch: u8,
    pub(crate) rx_signal: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) phase: Phase,
    pub(crate) frequency: u32,
    pub(crate) narrow: bool,
    pub(crate) am: bool,
    pub(crate) rssi: u32,
    pub(crate) noise: u32,
    pub(crate) audio: bool,
    pub(crate) monitor: bool,
    pub(crate) error: u32,
    pub(crate) signal: bool,
    pub(crate) glitch: u32,
    pub(crate) rx_signal: u16,
    pub(crate) tone_debug: [u16; 4],
    pub(crate) sample_number: u32,
}

#[repr(C)]
struct SquelchState {
    signature: u32,
    tick: u32,
    level: u32,
    open_rssi: u32,
    close_rssi: u32,
    open_noise: u32,
    close_noise: u32,
    open_glitch: u32,
    close_glitch: u32,
    rssi: u32,
    noise: u32,
    glitch: u32,
    signal: u32,
    bypass: u32,
    adjusted: u32,
    reserved: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_SQUELCH: SquelchState = unsafe { core::mem::zeroed() };

#[repr(C)]
struct ReceiverState {
    signature: u32,
    tick: u32,
    phase: u32,
    frequency: u32,
    rssi: u32,
    noise: u32,
    audio: u32,
    monitor: u32,
    narrow: u32,
    error: u32,
    samples: u32,
    retunes: u32,
    reg30: u32,
    reg33: u32,
    reg36: u32,
    filter_path: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_RECEIVER: ReceiverState = unsafe { core::mem::zeroed() };

pub(crate) struct Receiver {
    phase: Phase,
    preset: usize,
    deadline: u32,
    next_sample: u32,
    next_events: u32,
    vox_requested: u16,
    vox: super::vox::Detector,
    next_metrics: u32,
    next_check: u32,
    calibration: Option<Summary>,
    frequency: u32,
    narrow: bool,
    am: bool,
    monitor: bool,
    rssi: u32,
    noise: u32,
    audio: bool,
    error: u32,
    gate: Gate,
    samples: u32,
    last_sample: u32,
    retunes: u32,
    controls: [u16; 5],
    thresholds: Thresholds,
    squelch: u8,
    rx_signal: u16,
    tone: super::rx_signal::Gate,
    ptt: bool,
    glitch: u32,
    #[cfg(feature = "fsk")]
    fsk_enabled: bool,
    #[cfg(feature = "fsk")]
    fsk: super::fsk::Rx,
    #[cfg(feature = "cw")]
    cw_requested: u16,
    #[cfg(feature = "cw")]
    cw_pitch: u16,
    #[cfg(feature = "cw")]
    cw_original_3d: Option<u16>,
    #[cfg(feature = "cw")]
    cw_gated: bool,
    #[cfg(feature = "cw")]
    cw_threshold: u16,
    #[cfg(feature = "cw")]
    cw_gain: u8,
    #[cfg(feature = "cw")]
    cw_gain_requested: u8,
    #[cfg(feature = "cw")]
    cw_envelope: crate::cw_decode::Envelope,
}

impl Receiver {
    /// One foreground receiver shares the already initialized RF transport.
    /// It is constructed off and performs no register access here.
    pub(crate) fn new() -> Self {
        Self {
            phase: Phase::Off,
            preset: 0,
            deadline: 0,
            next_sample: 0,
            next_events: 0,
            vox_requested: 0,
            vox: super::vox::Detector::new(),
            next_metrics: 0,
            next_check: 0,
            calibration: None,
            frequency: Band::Vhf.initial(),
            narrow: false,
            am: false,
            monitor: false,
            rssi: 0,
            noise: 0,
            audio: false,
            error: 0,
            gate: Gate::new(),
            samples: 0,
            last_sample: 0,
            retunes: 0,
            controls: [0; 5],
            thresholds: Thresholds::disabled(),
            squelch: 0,
            rx_signal: 0,
            tone: super::rx_signal::Gate::new(),
            ptt: false,
            glitch: 0,
            #[cfg(feature = "fsk")]
            fsk_enabled: false,
            #[cfg(feature = "fsk")]
            fsk: super::fsk::Rx::new(),
            #[cfg(feature = "cw")]
            cw_requested: 0,
            #[cfg(feature = "cw")]
            cw_pitch: 0,
            #[cfg(feature = "cw")]
            cw_original_3d: None,
            #[cfg(feature = "cw")]
            cw_gated: false,
            #[cfg(feature = "cw")]
            cw_threshold: 160,
            #[cfg(feature = "cw")]
            cw_gain: 0,
            #[cfg(feature = "cw")]
            cw_gain_requested: 0,
            #[cfg(feature = "cw")]
            cw_envelope: crate::cw_decode::Envelope::new(),
        }
    }

    /// Zero selects ordinary reception. Nonzero selects a checked CW
    /// beat-frequency offset. gated selects only speaker admission; the
    /// independent carrier SQL still governs ordinary TX admission.
    /// This setter performs no register access. RF profile transitions
    /// occur while muted inside tune or park.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_select(&mut self, pitch: u16, gated: bool) {
        assert!(pitch == 0 || crate::cw::valid_pitch(pitch));
        self.cw_requested = pitch;
        self.cw_gated = gated;
    }

    /// Operational CW observation, not the carrier-admission gate. No chip
    /// read occurs here. Off, unsettled, failed, and input-muted reception
    /// cannot supply decoder observations.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_sample(&self) -> Option<(u32, bool)> {
        if self.cw_active() && matches!(self.phase, Phase::Receive) && !self.ptt {
            self.cw_envelope.observation()
        } else {
            None
        }
    }

    /// Read-only projection of the current CW receiver observations.
    /// Unsettled or input-muted reception has no diagnostic sample.
    #[cfg(feature = "cw-debug")]
    pub(crate) fn cw_probe(&self) -> Option<crate::cw_decode::Probe> {
        self.cw_sample()?;
        Some(self.cw_envelope.probe(
            self.rssi,
            self.cw_threshold as u32,
            self.cw_threshold.saturating_sub(4) as u32,
        ))
    }

    /// Selection only. Gain transitions occur muted through tune; changing
    /// the decoder threshold drops its partial envelope but does not rewrite
    /// the independent carrier-SQL thresholds or retune the synthesizer.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_options(&mut self, threshold: u16, gain: u8) {
        assert!(threshold >= 1 && threshold <= 511 && gain < 6);
        if self.cw_threshold != threshold {
            self.cw_threshold = threshold;
            self.cw_envelope.reset();
        }
        self.cw_gain_requested = gain;
    }

    /// Return observation validity and conservative CW carrier occupancy.
    ///
    /// Validity requires the completed normal RX settle/readback path,
    /// the selected CW pitch, AUTO gain, and an RSSI sample no older than
    /// twenty observed ticks. This getter reads no peripheral.
    ///
    /// Occupancy combines established carrier SQL with an immediate RSSI
    /// comparison against that detector's closing threshold. Using the
    /// lower hysteresis threshold avoids waiting for its opening timer
    /// before rejecting an already visible carrier.
    ///
    /// The independent user-adjusted decoder threshold does not participate.
    /// A reduced-gain or stale sample cannot establish channel vacancy.
    /// These observations do not predict activity after PA admission.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_admission(&self, now: u32) -> (bool, bool) {
        let valid = matches!(self.phase, Phase::Receive)
            && self.cw_pitch != 0
            && self.cw_pitch == self.cw_requested
            && self.cw_gain == 0
            && self.samples != 0
            && now.wrapping_sub(self.last_sample) <= 20;

        let occupied = self.gate.is_open()
            || self.rssi >= self.thresholds.close_rssi;

        (valid, occupied)
    }

    const fn cw_active(&self) -> bool {
        #[cfg(feature = "cw")]
        {
            self.cw_pitch != 0
        }
        #[cfg(not(feature = "cw"))]
        {
            false
        }
    }

    const fn audible_af(&self) -> u16 {
        if self.cw_active() { 0x6542 } else { 0x6142 }
    }

    fn tuned_frequency(&self) -> Result<u32, u32> {
        #[cfg(feature = "cw")]
        if self.cw_pitch != 0 {
            return crate::cw::rx_frequency(self.frequency, self.cw_pitch)
                .ok_or(0x7A01);
        }
        Ok(self.frequency)
    }

    /// NR7Y's BK4829 CW receive path uses AF_BASEBAND2 (selector five),
    /// REG_3D zero, and RX AF filtering 0500. Normal FM coefficient pairs
    /// and the existing narrow RF preset remain in use. No PCM is read.
    /// Preserve REG_3D because ordinary Trakt presets do not rebuild it.
    /// The caller has already muted audio and stopped RF links.
    #[cfg(feature = "cw")]
    fn cw_configure(&mut self) {
        if self.cw_pitch != 0 {
            if self.cw_original_3d.is_none() {
                self.cw_original_3d = Some(read(0x3D));
            }
            write(0x3D, 0);
            write(0x2B, 0x0500);
        } else {
            self.cw_restore();
        }
    }

    #[cfg(feature = "cw")]
    fn cw_restore(&mut self) {
        if let Some(original) = self.cw_original_3d.take() {
            write(0x3D, original);
            write(0x2B, 0);
        }
    }

    /// Select the optional FIFO listener without accessing the chip.
    /// Application dispatch sets this before RF service. Actual setup and
    /// cleanup remain inside pump, tune, and park under receiver ownership.
    #[cfg(feature = "fsk")]
    pub(crate) fn fsk_enabled(&mut self, enabled: bool) {
        self.fsk_enabled = enabled;
    }

    #[cfg(feature = "fsk")]
    pub(crate) fn fsk_view(&self) -> super::fsk::View {
        self.fsk.view()
    }

    /// Consume one checked Link packet after foreground RF service. This
    /// performs no hardware access and grants no transmit or save permission.
    #[cfg(feature = "fsk")]
    pub(crate) fn take_link_packet(&mut self) -> Option<crate::link::Packet> {
        self.fsk.take_packet()
    }

    /// The foreground must stop calling pump while TX owns the transport.
    /// Returning to RX starts a complete profile initialization from Off.
    pub(crate) fn suspend_for_tx(&mut self) {
        self.park();
        self.phase = Phase::Off;
    }

    /// Local feedback parks the links but retains the calibrated receiver
    /// object. Restore through the normal tune path, including its settling
    /// delay and fresh SQL history. No preset reset or PA admission occurs.
    pub(crate) fn resume_after_feedback(&mut self, now: u32) -> Result<(), u32> {
        if self.calibration.is_none() {
            return Err(0x7505);
        }

        self.tune(now)
    }

    /// Restore an unchanged CW receiver after verified transmitter cleanup.
    /// The caller owns parked RF and has stopped pumping the transmitter.
    ///
    /// TX changes frequency, links, PA, audio routing, and a small register
    /// subset. Its cleanup restores the captured voice-audio registers.
    /// The existing tune path rebuilds frequency, correction, bandwidth,
    /// demodulation, CW AF filters, signaling, LNA, and RX links. The complete
    /// preset table is not replayed and REG_00 is not reset.
    ///
    /// Reuse requires an exact saved profile, selected pitch, valid retained
    /// calibration, and Off phase. Other cases use ordinary full startup.
    /// The established fifty-tick settle and subsequent readback remain.
    /// SQL and decoder-envelope histories are reset by tune, not inherited.
    #[cfg(feature = "cw")]
    pub(crate) fn resume_cw(&mut self, now: u32, request: Request) {
        if matches!(self.phase, Phase::Off)
            && request.enabled
            && self.cw_pitch != 0
            && self.cw_pitch == self.cw_requested
            && self.frequency == request.frequency
            && self.narrow == request.narrow
            && self.am == request.am
            && self.rx_signal == request.rx_signal
            && self.calibration.is_some()
        {
            if let Err(error) = self.tune(now) {
                self.fail(error);
            }
        }
    }

    /// Selection performs no chip access. Zero disables operational VOX.
    /// Actual configuration runs inside the muted tune sequence.
    pub(crate) fn vox_select(&mut self, threshold: u16) {
        assert!(threshold <= 2047);
        // CW images retain no comparator owner, including during FM RX.
        self.vox_requested = if cfg!(feature = "cw") { 0 } else { threshold };
    }

    /// Selected-profile matching belongs to the detached TX Context.
    /// This projection additionally requires fresh settled FM metrics.
    /// Immediate occupancy uses all three closing conditions, not RSSI
    /// alone and not the independent VOX or signaling-audio latch.
    pub(crate) fn vox_admission(&self, now: u32) -> (bool, bool) {
        let valid = matches!(self.phase, Phase::Receive)
            && !self.am && !self.cw_active()
            && self.error == 0 && self.samples != 0
            && now.wrapping_sub(self.last_sample) <= 20;
        let occupied = self.gate.is_open()
            || (self.rssi >= self.thresholds.close_rssi
                && self.noise <= self.thresholds.close_noise
                && self.glitch <= self.thresholds.close_glitch);
        (valid, occupied)
    }

    pub(crate) fn vox_sample(&self, now: u32) -> Option<crate::vox::Sample> {
        if matches!(self.phase, Phase::Receive) {
            self.vox.sample(now)
        } else {
            None
        }
    }

    const fn vox_mask(&self) -> u16 {
        if self.vox_requested != 0 { super::vox::MASK } else { 0 }
    }

    pub(crate) fn owns_rf(&self) -> bool {
        matches!(self.phase, Phase::Program | Phase::Settle | Phase::Receive)
    }

    /// Construct the operational snapshot at one shared runtime boundary.
    /// This copies cached state only: no register read or RF transition.
    /// The caller owns the returned value; no receiver borrow is retained.
    #[inline(never)]
    pub(crate) fn view(&self) -> View {
        View {
            phase: self.phase,
            frequency: self.frequency,
            narrow: self.narrow,
            am: self.am,
            rssi: self.rssi,
            noise: self.noise,
            audio: self.audio,
            monitor: self.monitor,
            error: self.error,
            signal: matches!(self.phase, Phase::Receive) && self.gate.is_open(),
            rx_signal: self.rx_signal,
            tone_debug: self.tone.diagnostic(),
            glitch: self.glitch,
            sample_number: self.samples,
        }
    }

    fn park(&mut self) {
        self.vox.stop();
        #[cfg(feature = "fsk")]
        self.fsk.stop();

        amplifier(false);
        unsafe { inhibit() };
        write(0x37, 0x9D1F);
        write(0x47, 0x6042);
        self.audio = false;
        self.gate = Gate::new();
        self.tone = super::rx_signal::Gate::new();
        self.controls = [0, 0x9000, 0, 0x9D1F, 0x6042];

        #[cfg(feature = "cw")]
        {
            self.cw_restore();
            self.cw_envelope.reset();
        }
    }

    fn fail(&mut self, error: u32) {
        self.park();
        self.error = error;
        self.phase = Phase::Failed;
    }

    fn audio(&mut self, on: bool) -> Result<(), u32> {
        if self.audio == on {
            return Ok(());
        }

        if on {
            self.controls = verify_no_tx()?;
            configure(0x47, self.audible_af())?;
            amplifier(true);
        } else {
            amplifier(false);
            configure(0x47, 0x6042)?;
        }
        self.audio = on;
        Ok(())
    }

    fn begin(&mut self, request: Request, calibration: Summary) {
        self.park();
        write(0x00, 0x8000);
        write(0x00, 0);
        self.park();

        self.frequency = request.frequency;
        self.narrow = request.narrow;
        self.am = request.am;
        #[cfg(feature = "cw")]
        {
            self.cw_pitch = self.cw_requested;
        }
        self.calibration = Some(calibration);
        self.rx_signal = request.rx_signal;
        self.preset = 0;
        self.error = 0;
        self.phase = Phase::Program;
    }

    fn tune(&mut self, now: u32) -> Result<(), u32> {
        self.vox.stop();
        #[cfg(feature = "fsk")]
        self.fsk.stop();

        let band = match Band::receive(self.frequency) {
            Some(band) => band,
            None => return Err(0x5003),
        };
        let calibration = match self.calibration {
            Some(calibration) => calibration,
            None => return Err(0x5001),
        };

        self.audio(false)?;
        configure(0x30, 0)?;
        configure(0x33, 0x9000)?;
        configure(0x36, 0)?;
        configure(0x47, 0x6042)?;

        let frequency = self.tuned_frequency()? / 10;
        configure(0x38, frequency as u16)?;
        configure(0x39, (frequency >> 16) as u16)?;
        configure(0x43, super::demod::bandwidth(self.am, self.narrow))?;
        configure(0x3B, (22_656i32 + calibration.crystal as i32) as u16)?;

        configure(0x48, audio_gain(calibration.volume, calibration.dac))?;
        super::demod::configure(self.am);
        #[cfg(feature = "cw")]
        {
            self.cw_configure();
            self.cw_gain = if self.cw_pitch != 0 { self.cw_gain_requested } else { 0 };
            super::rx_gain::configure(self.cw_gain);
        }

        super::rx_signal::configure(self.rx_signal, self.vox_mask())?;
        if self.vox_requested != 0 {
            if self.am || self.cw_active() {
                return Err(0x7C02);
            }
            self.vox.start(now, self.vox_requested)?;
        }
        self.next_events = now;

        // Select one LNA with RX-enable asserted. PA GPIO1 remains low.
        configure(0x33, band.gpio())?;
        configure(0x37, 0x9F1F)?;
        configure(0x30, 0xBFF1)?;

        self.gate = Gate::new();
        self.tone = super::rx_signal::Gate::new();
        #[cfg(feature = "cw")]
        self.cw_envelope.reset();
        self.rssi = 0;
        self.noise = 0;
        self.next_metrics = now;
        self.deadline = now.wrapping_add(50);
        self.phase = Phase::Settle;
        self.retunes = self.retunes.saturating_add(1);
        Ok(())
    }

    fn check(&mut self) -> Result<(), u32> {
        let band = match Band::receive(self.frequency) {
            Some(band) => band,
            None => return Err(0x5003),
        };
        self.controls = verify_no_tx()?;

        if self.controls[0] & 0x7FFF != 0x3FF1
            || self.controls[1] != band.gpio()
            || self.controls[3] != 0x9F1F
            || self.controls[4] != if self.audio { self.audible_af() } else { 0x6042 }
        {
            return Err(0x5002);
        }

        let actual = read(0x38) as u32 | ((read(0x39) as u32) << 16);
        if actual != self.tuned_frequency()? / 10 {
            return Err(0x5038);
        }
        if read(0x43) != super::demod::bandwidth(self.am, self.narrow) {
            return Err(0x5043);
        }

        let calibration = match self.calibration {
            Some(calibration) => calibration,
            None => return Err(0x5001),
        };
        if read(0x3B) != (22_656i32 + calibration.crystal as i32) as u16 {
            return Err(0x503B);
        }
        super::demod::check(self.am, self.vox.active())?;
        self.vox.check()?;

        #[cfg(feature = "cw")]
        super::rx_gain::check(self.cw_gain)?;

        #[cfg(feature = "cw")]
        if self.cw_pitch != 0
            && (self.am || self.rx_signal != 0
                || read(0x3D) != 0 || read(0x2B) != 0x0500)
        {
            return Err(0x7A02);
        }

        #[cfg(feature = "fsk")]
        if self.fsk.armed() {
            if self.am || self.rx_signal != 0 || read(0x51) != 0 {
                return Err(0x7802);
            }
            self.fsk.check()?;
        } else {
            super::rx_signal::check(self.rx_signal, self.vox_mask())?;
        }

        #[cfg(not(feature = "fsk"))]
        super::rx_signal::check(self.rx_signal, self.vox_mask())?;

        Ok(())
    }

    /// Call once per observed tick after processing keys.
    /// PTT suppresses audio but never changes RX links to a TX state.
    /// keep_carrier is used only for muted selected-VFO PTT qualification:
    /// SQL history continues while the final audio gate still receives PTT.
    /// It neither bypasses SQL nor treats a decoded tone as channel vacancy.
    #[inline(never)]
    pub(crate) fn pump(
        &mut self,
        now: u32,
        request: Request,
        memory: MemoryView,
        thresholds: Result<Thresholds, u32>,
        ptt: bool,
        keep_carrier: bool,
    ) {
        // Normalize operational signaling, not the saved profile. An AM
        // request cannot accidentally retain a tone-controlled audio gate.
        let mut request = request;
        if request.am {
            request.rx_signal = 0;
        }

        #[cfg(feature = "cw")]
        let thresholds = if self.cw_requested != 0 {
            // CW bypasses FM noise/glitch classification. Retaining only
            // the factory FM RSSI pair would make that composite detector
            // more permissive than its calibrated six-column definition.
            //
            // Use the existing explicit AM RSSI scale for carrier admission:
            // SQL 1..9 opens at 160 + 16 * (level - 1), closes eight counts
            // below it. This selects thresholds only, not AM demodulation.
            // Preserve any supplied acquisition or threshold error.
            //
            // The scale is an operating policy, not a measured noise floor
            // or proof of channel vacancy. CW audio remains independently
            // open; transmitter admission still rejects detected carrier.
            thresholds.and_then(|_| Thresholds::am(request.squelch))
        } else {
            thresholds
        };


        if !request.enabled {
            if !matches!(self.phase, Phase::Off) {
                self.park();
                self.phase = Phase::Off;
                self.error = 0;
            }
            self.frequency = request.frequency;
            self.narrow = request.narrow;
            self.am = request.am;
            self.monitor = request.monitor;
            self.rx_signal = request.rx_signal;
            self.ptt = ptt;
            return;
        }
        if matches!(self.phase, Phase::Failed) {
            return;
        }
        if Band::receive(request.frequency).is_none() {
            self.fail(0x5003);
            return;
        }
        if !crate::tx_signal::valid(request.rx_signal as u32) {
            self.fail(0x5104);
            return;
        }

        if matches!(self.phase, Phase::Off | Phase::Wait) {
            self.frequency = request.frequency;
            self.narrow = request.narrow;
            self.am = request.am;
            self.phase = Phase::Wait;

            if memory.error != 0 {
                self.fail(0x5004);
                return;
            }
            if !memory.ready {
                return;
            }

            let calibration = match memory.calibration {
                Some(calibration) if calibration.flags & 5 == 5 => calibration,
                _ => {
                    self.fail(0x5001);
                    return;
                }
            };
            self.thresholds = match thresholds {
                Ok(thresholds) => thresholds,
                Err(error) => {
                    self.fail(error);
                    return;
                }
            };
            self.squelch = request.squelch;
            self.monitor = request.monitor;
            self.ptt = ptt;
            self.begin(request, calibration);
            return;
        }

        #[cfg(feature = "cw")]
        if (self.cw_pitch != self.cw_requested
            || self.cw_gain != if self.cw_requested != 0 {
                self.cw_gain_requested
            } else {
                0
            })
            && matches!(self.phase, Phase::Settle | Phase::Receive)
        {
            if let Err(error) = self.audio(false) {
                self.fail(error);
                return;
            }
            self.cw_pitch = self.cw_requested;
            self.frequency = request.frequency;
            self.narrow = request.narrow;
            self.am = request.am;
            if let Err(error) = self.tune(now) {
                self.fail(error);
            }
            return;
        }

        if matches!(self.phase, Phase::Settle | Phase::Receive)
            && !self.vox.matches(self.vox_requested)
        {
            if let Err(error) = self.tune(now) {
                self.fail(error);
            }
            return;
        }

        let configured = match thresholds {
            Ok(thresholds) => thresholds,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        if self.thresholds != configured || self.squelch != request.squelch
            || self.monitor != request.monitor || self.ptt != ptt
            || self.rx_signal != request.rx_signal
        {
            if let Err(error) = self.audio(false) {
                self.fail(error);
                return;
            }
            let reset_tone = self.rx_signal != 0 || request.rx_signal != 0;
            self.thresholds = configured;
            self.squelch = request.squelch;
            self.monitor = request.monitor;
            self.rx_signal = request.rx_signal;
            self.ptt = ptt;
            self.gate = Gate::new();
            self.next_sample = now;

            // Leaving bypass or changing a tone must reacquire hardware
            // status, not inherit the preceding matched-tone latch.
            if reset_tone && matches!(self.phase, Phase::Settle | Phase::Receive) {
                self.frequency = request.frequency;
                self.narrow = request.narrow;
                self.am = request.am;
                if let Err(error) = self.tune(now) {
                    self.fail(error);
                }
                return;
            }
        }

        if matches!(self.phase, Phase::Program) {
            if self.preset < PRESETS.len() {
                let (register, value) = PRESETS[self.preset];
                match configure(register, value) {
                    Ok(()) => self.preset += 1,
                    Err(error) => self.fail(error),
                }
                return;
            }
            self.frequency = request.frequency;
            self.narrow = request.narrow;
            self.am = request.am;
            if let Err(error) = self.tune(now) {
                self.fail(error);
            }
            return;
        }

        if self.frequency != request.frequency || self.narrow != request.narrow
            || self.am != request.am
        {
            self.frequency = request.frequency;
            self.narrow = request.narrow;
            self.am = request.am;
            if let Err(error) = self.tune(now) {
                self.fail(error);
            }
            return;
        }

        if matches!(self.phase, Phase::Settle) {
            if reached(now, self.deadline) {
                if let Err(error) = self.check() {
                    self.fail(error);
                    return;
                }
                self.phase = Phase::Receive;
                self.next_sample = now;
                self.next_check = now.wrapping_add(1_000);
            } else {
                return;
            }
        }

        if ptt && self.audio {
            if let Err(error) = self.audio(false) {
                self.fail(error);
                return;
            }
        }

        if reached(now, self.next_check) {
            if let Err(error) = self.check() {
                self.fail(error);
                return;
            }
            self.next_check = now.wrapping_add(1_000);
        }

        // Tone and VOX share acknowledgement. RSSI/noise/glitch retain
        // their own sampling cadence. FSK has neither of these consumers.
        if (self.rx_signal != 0 || self.vox.active())
            && reached(now, self.next_events)
        {
            if self.rx_signal != 0 && self.tone.stale(now) {
                if let Err(error) = self.tune(now) {
                    self.fail(error);
                }
                return;
            }
            let events = self.tone.poll(now, self.rx_signal);
            self.vox.observe(now, events);
            self.next_events = now.wrapping_add(
                if self.vox.active() { 10 } else { 20 },
            );
        }

        if reached(now, self.next_sample) {
            let carrier_was_open = self.gate.is_open();
            self.rssi = (read(0x67) & 0x01FF) as u32;
            if !self.cw_active() || reached(now, self.next_metrics) {
                self.noise = (read(0x65) & 0x007F) as u32;
                self.glitch = (read(0x63) & 0x00FF) as u32;
                self.next_metrics = now.wrapping_add(20);
            }
            self.samples = self.samples.saturating_add(1);
            self.last_sample = now;
            self.next_sample = now.wrapping_add(
                if self.cw_active() { 5 } else { 20 },
            );

            #[cfg(feature = "cw")]
            if self.cw_active() {
                if ptt && !keep_carrier {
                    self.cw_envelope.reset();
                } else {
                    self.cw_envelope.poll(
                        now, self.rssi,
                        self.cw_threshold as u32,
                        self.cw_threshold.saturating_sub(4) as u32,
                    );
                }
            }
            let open = self.gate.update(
                now,
                Metrics {
                    rssi: self.rssi,
                    // The fast CW envelope does not replace this carrier
                    // admission history. FM noise/glitch cannot classify
                    // a CW carrier; their raw observations remain visible.
                    noise: if self.cw_active() { 0 } else { self.noise },
                    glitch: if self.cw_active() { 0 } else { self.glitch },
                },
                self.thresholds,
                request.monitor || request.squelch == 0,
                ptt && !keep_carrier,
            );
            if carrier_was_open && !self.gate.is_open() {
                self.tone.carrier_closed();
            }
            let audio = if self.cw_active() {
                #[cfg(feature = "cw")]
                {
                    !ptt && (!self.cw_gated || self.cw_envelope.audible(now))
                }
                #[cfg(not(feature = "cw"))]
                {
                    false
                }
            } else {
                self.tone.audio(
                    open, self.rx_signal != 0, request.monitor, ptt,
                )
            };
            if let Err(error) = self.audio(audio) {
                self.fail(error);
            }
        }

        #[cfg(feature = "fsk")]
        if matches!(self.phase, Phase::Receive) {
            let enabled = self.fsk_enabled && !self.am
                && self.rx_signal == 0 && !self.cw_active();
            if let Err(error) = self.fsk.pump(now, enabled) {
                self.fail(error);
            }
        }
    }

    pub(crate) fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let state = ReceiverState {
            signature: 0,
            tick: now,
            phase: self.phase as u32,
            frequency: self.frequency,
            rssi: self.rssi,
            noise: self.noise,
            audio: self.audio as u32,
            monitor: self.monitor as u32,
            narrow: self.narrow as u32,
            error: self.error,
            samples: self.samples,
            retunes: self.retunes,
            reg30: self.controls[0] as u32,
            reg33: self.controls[1] as u32,
            reg36: self.controls[2] as u32,
            filter_path: (self.controls[1] & 0x000C) as u32,
        };
        crate::observation::publish!(TRAKT_RECEIVER, state, 0x5452_4354);

        let sql = SquelchState {
            signature: 0,
            tick: now,
            level: self.squelch as u32,
            open_rssi: self.thresholds.open_rssi,
            close_rssi: self.thresholds.close_rssi,
            open_noise: self.thresholds.open_noise,
            close_noise: self.thresholds.close_noise,
            open_glitch: self.thresholds.open_glitch,
            close_glitch: self.thresholds.close_glitch,
            rssi: self.rssi,
            noise: self.noise,
            glitch: self.glitch,
            signal: self.view().signal as u32,
            bypass: (self.monitor || self.squelch == 0) as u32,
            adjusted: self.thresholds.adjusted as u32,
            reserved: 0,
        };
        crate::observation::publish!(TRAKT_SQUELCH, sql, 0x5451_4354);
    }
}

const _: () = {
    assert!(core::mem::size_of::<ReceiverState>() == 64);

    assert!(audio_gain(55, 14) == 0xB37E);
    assert!(audio_gain(0, 0) == 0xB000);
    assert!(audio_gain(63, 15) == 0xB3FF);

    let mut volume = 0u8;
    while volume < 64 {
        let mut dac = 0u8;
        while dac < 16 {
            let value = audio_gain(volume, dac);
            assert!(value & 0xF000 == 0xB000);
            assert!(value & 0x0C00 == 0);
            assert!((value >> 4) & 63 == volume as u16);
            assert!(value & 15 == dac as u16);
            dac += 1;
        }
        volume += 1;
    }

    assert!(matches!(Band::of(136_000_000), Some(Band::Vhf)));
    assert!(matches!(Band::of(174_000_000), Some(Band::Vhf)));
    assert!(matches!(Band::of(400_000_000), Some(Band::Uhf)));
    assert!(matches!(Band::of(470_000_000), Some(Band::Uhf)));
    assert!(Band::of(135_999_990).is_none());
    assert!(Band::of(174_000_010).is_none());
    assert!(Band::of(280_000_000).is_none());
    assert!(Band::of(399_999_990).is_none());
    assert!(Band::of(470_000_010).is_none());
    assert!(Band::of(433_500_001).is_none());
    assert!(Band::of(u32::MAX).is_none());

    assert!(Band::Vhf.gpio() == 0x9044);
    assert!(Band::Uhf.gpio() == 0x9048);
    assert!(admitted_write(0x33, Band::Vhf.gpio()));
    assert!(admitted_write(0x33, Band::Uhf.gpio()));
    assert!(!admitted_write(0x33, 0x904C));
    assert!(!admitted_write(0x33, 0x9068));
    assert!(!admitted_write(0x33, 0x9049));

    assert!(Band::Vhf.shift(174_000_000, 12_500, true) == 174_000_000);
    assert!(Band::Vhf.shift(136_000_000, 12_500, false) == 136_000_000);
    assert!(Band::Uhf.shift(470_000_000, 12_500, true) == 470_000_000);
    assert!(Band::Uhf.shift(400_000_000, 12_500, false) == 400_000_000);
    assert!(Band::Uhf.shift(469_999_990, 12_500, true) == 470_000_000);
    assert!(Band::Uhf.shift(400_000_010, 12_500, false) == 400_000_000);
    assert!(Band::Uhf.shift(433_500_000, 12_500, true) == 433_512_500);
    assert!(Band::Uhf.shift(433_500_000, 12_500, false) == 433_487_500);
    assert!(Band::Uhf.shift(433_500_000, 4_000_000_000, true) == 470_000_000);
    assert!(Band::Uhf.shift(433_500_000, 4_000_000_000, false) == 400_000_000);
    assert!(admitted_write(0x30, 0xBFF1));
    assert!(!admitted_write(0x30, 0xC1FE));
    assert!(!admitted_write(0x30, 0x0302));
    assert!(!admitted_write(0x36, 0x0080));
    assert!(!admitted_write(0x33, 0x9064));

    let mut index = 0;
    while index < PRESETS.len() {
        assert!(admitted_write(PRESETS[index].0, PRESETS[index].1));
        index += 1;
    }

};
