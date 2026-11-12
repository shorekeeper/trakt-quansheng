//! Foreground TX ownership with normal PTT and independent admission.
//!
//! CW compositions omit operational VOX even when their VFO uses FM.
//! Automatic voice acquisition, comparator handoff, VOX tail selection,
//! and comparator sampling have no runtime path in those images. Manual
//! voice, CW, practice, and any compiled FSK ownership remain independent.
//! Persistent VOX choices are data only and cannot enable those paths.
//!
//! A new debounced PTT edge requires at least 100 observed ticks of release.
//! Holding PTT at boot, through a timeout, or after an error cannot restart TX.
//! Every new attempt checks UI scope, policy, receive settling, busy SQL,
//! calibration, and a fresh battery sample. RX settling and busy SQL refer
//! to the original receive profile. Policy, calibration, and RF setup use
//! the resolved TX frequency. Completion resumes the original RX request.
//! F does not arm transmission.
//!
//! Dual Watch adds a bounded selected-VFO acquisition phase after a qualified
//! PTT edge. RX remains muted while fresh carrier samples are collected.
//! The acquisition phase cannot admit PA or a Roger tail. Successful setup
//! retains the original attempt start, so acquisition does not extend TOT.
//! The SysTick backstop is armed before TX setup using that same start time.
//!
//! CW may queue a fresh element while the receiver is restoring its profile.
//! Its acquisition waits for the matching settled receiver and a fresh
//! AUTO-gain RSSI observation. Established carrier SQL or immediate RSSI at
//! its closing threshold rejects TX. The separate decoder threshold cannot
//! admit RF. No extra Dual Watch quiet dwell follows that observation.
//! Acquisition and setup remain inside the original complete-session TOT.
//! Dual Watch retains its independent advancing-sample quiet qualification.
//!
//! Normal CW hang completion can carry a prior 100-tick physical release
//! observation. It is accepted only from On in the exact complete space
//! state, after ordinary RF-off cleanup succeeds. This removes a redundant
//! post-hang release delay without renewing an active attempt. Error, timeout,
//! changed scope, and acquisition cancellation retain release locking.
//!
//! HAM override, power, and TOT are frozen for one attempt. A changed request
//! cancels it. All profiles use the same checked register sequencing.
//! MAX uses the unmodified factory High calibration. Names are not watts.
//!
//! Local permit and refusal cues use AF/DSP with PLL and PA disabled.
//! Roger runs only after normal release, with MIC ADC disabled, and only when
//! at least 600 ticks remain inside the original selected TOT deadline.
//! EXIT, timeout, battery failure, and other cancellation paths have no tail.
//!
//! PA drive, PA gate, and RF links are removed before sound cleanup.
//! RF-off readback precedes clearing the SysTick guard. Failure to establish
//! the off state enters the common trap instead of returning to reception.
//!
//! With fam compiled and SlopeCast enabled, an admitted AM simplex request
//! uses shifted FM TX. Both nominal and actual frequencies pass TX policy;
//! factory calibration is selected using the actual RF frequency. The RX
//! frequency, AM selector, RX width, TX width, and SlopeCast settings remain
//! frozen for one attempt. The saved receive profile is never rewritten.
//!
//! SlopeCast disables transmitted subaudio and Roger, retaining local permit
//! behavior. It explicitly restores FM demodulator coefficients while RF is
//! parked, before the ordinary PA admission steps. Completion uses the normal
//! RF-off cleanup and the receiver's complete AM initialization.
//! Diagnostic frequency is actual RF; the operational view retains nominal
//! frequency with a distinct SC-FM marker.
//!
//! CW sidetone uses AF_ALAM and AF DAC under this transmitter's ownership.
//! Tone1 remains enabled through a session; its frequency word follows
//! completed RF mark/space edges. MIC ADC stays disabled and the established
//! TX modulation-mute command remains in place. Silent local audio begins
//! before RF programming. Its seventy-tick settling deadline overlaps RF
//! preparation rather than following it. cw_ready requires both completed
//! RF checks and that original deadline; element timing starts afterward.
//! RF edge order, calibrated PA code, busy admission, and TOT are unchanged.
//! Shutdown removes RF and speaker output before the tone owner restores
//! its captured frequency word. Local register retention is not an RF test.
//!
//! Optional full break-in uses a separate transmitter-owned receive-window
//! object. Initial admission and complete TX setup remain unchanged. A falling
//! mark removes PA through the ordinary CW edge, then selects shifted RX.
//! The requested RX wait controls speaker admission, not the logical session
//! hang. A following electronic mark requests checked TX restoration before
//! its duration starts. Straight release remains live.
//!
//! Receiver and storage owners stay suspended during these windows. Captured
//! RX/TX words, exact control tuples, and fresh post-write deadlines bound the
//! transitions. The session's original TOT and SysTick cutoff remain armed;
//! a receive window cannot refresh them or inherit permission after a failure.
//! Speaker windows do not provide continuous RX decoder history.
//!
//! A refusal or failed attempt starts a 2000-tick UI notification. Its
//! expiry hides the displayed error without clearing the diagnostic code,
//! changing PTT release qualification, replaying alert audio, or granting TX.
//! Each actual new attempt can produce its own notification. A held PTT
//! cannot refresh the notification merely by remaining asserted.



use super::{read, write, verify, verify_transmit};
use super::receive::{Phase as RxPhase, Receiver, Request, View as RxView};
use super::sound::{Local, Tail};
use crate::app::{
    sound::Settings as SoundSettings,
    time::reached,
    tx_policy,
};
use crate::drivers::adc::Reading;
use crate::mcu;
use crate::storage::transmit::Profile;
use crate::startup;

const MIC_CONFIG: u16 = 0xE940;

struct Release {
    ready: bool,
    blocked: bool,
    observing: bool,
    deadline: u32,
}

impl Release {
    const fn new() -> Self {
        Self { ready: false, blocked: true, observing: false, deadline: 0 }
    }

    const fn sample(&mut self, now: u32, held: bool) {
        if held {
            // Raw assertion precedes debounced PTT. Preserve the completed
            // release qualification until begin or reject consumes it.
            // A held input still cannot complete a new release interval.
            self.observing = false;
        } else {
            if !self.observing {
                self.observing = true;
                self.deadline = now.wrapping_add(100);
            }
            if reached(now, self.deadline) {
                self.ready = true;
                self.blocked = false;
            }
        }
    }

    const fn lock(&mut self) {
        *self = Self::new();
    }

    /// The caller has completed normal CW RF-off cleanup and holds a
    /// one-pass observation of both physical contacts released for 100 ticks.
    /// This accepts that existing observation; it does not time a new one.
    /// No error, timeout, source change, or acquisition cancellation calls it.
    #[cfg(feature = "cw")]
    const fn accept_cw_release(&mut self, now: u32) {
        self.ready = true;
        self.blocked = false;
        self.observing = true;
        self.deadline = now;
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Alert,
    Permit,
    Setup,
    Delay,
    On,
    Tail,
    Acquire,
    #[cfg(feature = "cw")]
    Practice,
}

enum Step {
    Write(u8, u16),
    Microphone,
    Subtone,
    Fm,
    Wait(u32),
    Controls,
    Check,
    #[cfg(feature = "cw")]
    CwAudio,
    Finish,
}

/// One foreground attempt observation, owned by the radio task.
/// pump borrows it for one call rather than moving another aggregate into
/// the transmitter. No reference is retained. Automatic FSK dispatch may
/// normalize its admission fields inside this caller-owned temporary;
/// persistent choices and the transmitter's frozen attempt remain separate.
pub(crate) struct Context {
    #[cfg(feature = "cw")]
    pub(crate) cw_pitch: u16,
    #[cfg(feature = "cw")]
    pub(crate) cw_down: bool,
    #[cfg(feature = "cw")]
    pub(crate) cw_prepare: bool,
    #[cfg(feature = "cw")]
    pub(crate) cw_complete: bool,
    #[cfg(feature = "cw")]
    pub(crate) cw_qsk: bool,
    #[cfg(feature = "cw")]
    pub(crate) cw_volume: u8,
    #[cfg(feature = "cw")]
    pub(crate) cw_breakin: u8,
    #[cfg(feature = "cw")]
    pub(crate) cw_tail: u8,
    #[cfg(feature = "fsk")]
    pub(crate) fsk_status: u8,
    #[cfg(feature = "fsk")]
    pub(crate) fsk_message: Option<crate::link::Message>,
    #[cfg(feature = "fsk")]
    pub(crate) fsk_ack: Option<crate::link::Packet>,
    #[cfg(feature = "fsk")]
    pub(crate) fsk_ack_allowed: bool,
    pub(crate) vox_request: bool,
    pub(crate) vox_complete: bool,
    pub(crate) vox_threshold: u16,
    pub(crate) voice: crate::voice::Settings,
    pub(crate) dual_watch: bool,
    pub(crate) slope: crate::slope::Settings,
    pub(crate) transmit: Result<u32, u32>,
    pub(crate) tx_signal: u16,
    pub(crate) allowed: bool,
    pub(crate) request: Request,
    pub(crate) receiver: RxView,
    pub(crate) calibration: Result<Profile, u32>,
    pub(crate) battery: Reading,
    pub(crate) ptt: bool,
    pub(crate) raw_ptt: bool,
    pub(crate) ham: bool,
    pub(crate) power: u8,
    pub(crate) tot: u32,
}

impl Context {
    const fn tx_narrow(&self) -> bool {
        if self.request.am { self.slope.narrow } else { self.request.narrow }
    }

    const fn receiver_settled(&self) -> bool {
        matches!(self.receiver.phase, RxPhase::Receive)
            && self.receiver.frequency == self.request.frequency
            && self.receiver.narrow == self.request.narrow
            && self.receiver.am == self.request.am
            && self.receiver.rx_signal == self.request.rx_signal
    }

    /// Dual Watch always acquires selected RX and observes its quiet interval.
    /// CW uses acquisition only while its supplied RX observation is invalid
    /// or unsettled. Receiver validity includes AUTO gain and sample freshness.
    /// A settled busy channel is refused without waiting for a quiet interval.
    const fn acquisition_required(&self) -> bool {
        if self.dual_watch || (!cfg!(feature = "cw") && self.vox_request) {
            return true;
        }

        #[cfg(feature = "cw")]
        {
            self.cw_pitch != 0 && !self.receiver_settled()
        }

        #[cfg(not(feature = "cw"))]
        {
            false
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct View {
    pub(crate) active: bool,
    pub(crate) slope: bool,
    pub(crate) blocked: bool,
    pub(crate) status: &'static [u8],
    pub(crate) frequency: u32,
    pub(crate) narrow: bool,
    pub(crate) elapsed: u32,
    pub(crate) budget: u32,
    pub(crate) bias: u32,
    pub(crate) error: u32,
    pub(crate) carrier: bool,
    #[cfg(feature = "cw")]
    pub(crate) keyed: bool,
    pub(crate) ham: bool,
    pub(crate) power: u8,
    pub(crate) blink: bool,
}

#[repr(C)]
struct TransmitState {
    signature: u32,
    tick: u32,
    phase: u32,
    ham: u32,
    frequency: u32,
    bias: u32,
    pa: u32,
    elapsed: u32,
    error: u32,
    starts: u32,
    timeouts: u32,
    reg30: u32,
    reg33: u32,
    reg36: u32,
    checks: u32,
    audio_readback: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_TRANSMITTER: TransmitState = unsafe { core::mem::zeroed() };

pub(crate) struct Transmitter {
    #[cfg(feature = "cw")]
    cw_pitch: u16,
    #[cfg(feature = "cw")]
    cw_qsk: bool,
    #[cfg(feature = "cw")]
    cw_volume: u8,
    #[cfg(feature = "cw")]
    cw_audio_at: u32,
    #[cfg(feature = "cw")]
    sidetone: super::cw_audio::Sidetone,
    #[cfg(feature = "cw")]
    cw_breakin: u8,
    #[cfg(feature = "cw")]
    cw_tail: u8,
    #[cfg(feature = "cw")]
    breakin: super::cw_breakin::BreakIn,
    #[cfg(feature = "fsk")]
    fsk_status: u8,
    #[cfg(feature = "fsk")]
    fsk_message: Option<crate::link::Message>,
    #[cfg(feature = "fsk")]
    automatic_ack: Option<crate::link::Packet>,
    #[cfg(feature = "fsk")]
    ack_completed: bool,
    #[cfg(feature = "fsk")]
    ack_result: Option<(crate::link::Packet, bool)>,
    vox_attempt: bool,
    previous_vox: bool,
    vox_threshold: u16,
    vox_finished: bool,
    vox: super::vox::Detector,
    voice: crate::voice::Settings,
    processing: super::voice::Processing,
    release: Release,
    acquisition: crate::app::ptt_check::Check,
    phase: Phase,
    profile: Profile,
    receive_frequency: u32,
    receive_am: bool,
    receive_narrow: bool,
    slope: crate::slope::Settings,
    receive_signal: u16,
    frequency: u32,
    tx_signal: u16,
    narrow: bool,
    previous_ptt: bool,
    step: usize,
    wake_at: u32,
    started: u32,
    deadline: u32,
    next_check: u32,
    now: u32,
    original_audio: [u16; 3],
    expected: [u16; 5],
    error: u32,
    starts: u32,
    timeouts: u32,
    checks: u32,
    audio_expected: u16,
    audio_actual: u16,
    notice: crate::app::tx_notice::Notice,
    sounds: SoundSettings,
    local: Local,
    roger: Tail,
    ham: bool,
    power: u8,
    budget: u32,
}

impl Transmitter {
    pub(crate) const fn new() -> Self {
        Self {
            #[cfg(feature = "cw")]
            cw_pitch: 0,
            #[cfg(feature = "cw")]
            cw_qsk: true,
            #[cfg(feature = "cw")]
            cw_volume: 28,
            #[cfg(feature = "cw")]
            cw_audio_at: 0,
            #[cfg(feature = "cw")]
            sidetone: super::cw_audio::Sidetone::new(),
            #[cfg(feature = "cw")]
            cw_breakin: 0,
            #[cfg(feature = "cw")]
            cw_tail: 0,
            #[cfg(feature = "cw")]
            breakin: super::cw_breakin::BreakIn::new(),
            #[cfg(feature = "fsk")]
            fsk_status: 0,
            #[cfg(feature = "fsk")]
            fsk_message: None,
            #[cfg(feature = "fsk")]
            automatic_ack: None,
            #[cfg(feature = "fsk")]
            ack_completed: false,
            #[cfg(feature = "fsk")]
            ack_result: None,
            vox_attempt: false,
            previous_vox: false,
            vox_threshold: 0,
            vox_finished: false,
            vox: super::vox::Detector::new(),
            voice: crate::voice::Settings::new(),
            processing: super::voice::Processing::new(),
            release: Release::new(),
            acquisition: crate::app::ptt_check::Check::new(0),
            phase: Phase::Idle,
            profile: Profile::empty(),
            receive_frequency: 0,
            receive_am: false,
            receive_narrow: false,
            slope: crate::slope::Settings::new(),
            receive_signal: 0,
            frequency: 0,
            tx_signal: 0,
            narrow: false,
            previous_ptt: false,
            step: 0,
            wake_at: 0,
            started: 0,
            deadline: 0,
            next_check: 0,
            now: 0,
            original_audio: [0; 3],
            expected: [0, 0x9000, 0, 0x9D1F, 0x6042],
            error: 0,
            starts: 0,
            timeouts: 0,
            checks: 0,
            audio_expected: 0,
            audio_actual: 0,
            notice: crate::app::tx_notice::Notice::new(),
            sounds: SoundSettings::new(),
            local: Local::new(),
            roger: Tail::new(),
            ham: true,
            power: 0,
            budget: 30000,
        }
    }

    /// Automatic responses use physical PTT only for cancellation. They do
    /// not create a synthetic press or reuse the remote sender's TX permit.
    const fn automatic(&self) -> bool {
        #[cfg(feature = "fsk")]
        { self.automatic_ack.is_some() }
        #[cfg(not(feature = "fsk"))]
        { false }
    }

    #[cfg(feature = "fsk")]
    pub(crate) const fn ack_active(&self) -> bool {
        self.automatic_ack.is_some()
    }

    /// Consume one response outcome after RF service. True means local
    /// modem completion, not remote receipt of the acknowledgement.
    #[cfg(feature = "fsk")]
    pub(crate) fn take_ack_result(&mut self) -> Option<(crate::link::Packet, bool)> {
        self.ack_result.take()
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

    const fn cw_unchanged(&self, _context: &Context) -> bool {
        #[cfg(feature = "cw")]
        {
            self.cw_pitch == _context.cw_pitch
                && self.cw_qsk == _context.cw_qsk
                && self.cw_volume == _context.cw_volume
                && self.cw_breakin == _context.cw_breakin
                && self.cw_tail == _context.cw_tail
        }
        #[cfg(not(feature = "cw"))]
        {
            true
        }
    }

    const fn training(&self) -> bool {
        #[cfg(feature = "cw")]
        {
            self.cw_pitch != 0 && !self.cw_qsk
        }
        #[cfg(not(feature = "cw"))]
        {
            false
        }
    }

    #[cfg(feature = "cw")]
    fn mark_state(&self) -> bool {
        if self.training() {
            return matches!(self.phase, Phase::Practice)
                && reached(self.now, self.wake_at)
                && self.sidetone.down();
        }

        self.engaged() && self.expected[2] != 0
    }

    const fn cw_audio_active(&self) -> bool {
        #[cfg(feature = "cw")]
        {
            self.sidetone.active()
        }
        #[cfg(not(feature = "cw"))]
        {
            false
        }
    }

    const fn tone_control(&self) -> u16 {
        #[cfg(feature = "cw")]
        { self.sidetone.control() }
        #[cfg(not(feature = "cw"))]
        { 0 }
    }

    /// Automatic voice retains the complete microphone configuration under
    /// which its RX comparator observations were obtained. The ordinary
    /// receiver preset supplies this word; begin captures it after RF parking
    /// and before local permit or TX setup.
    ///
    /// Replacing E920 with the manual-TX E940 word changes microphone
    /// configuration at the detector handoff. No undocumented gain conversion
    /// is assumed here: retain the complete captured word and verify it.
    /// Manual PTT, CW, and packet transmission keep their established preset.
    /// This changes no MIC ADC, RF-link, PA, audio-mute, or deadline controls.
    const fn microphone_config(&self) -> u16 {
        if !cfg!(feature = "cw") && self.vox_attempt {
            self.original_audio[0]
        } else {
            MIC_CONFIG
        }
    }

    const fn tx_links(&self) -> u16 {
        // CW retains the prepared AF DAC while enabling its synthesizer
        // and TX DSP. MIC ADC and internal PA gain remain disabled.
        // Voice keeps the established full TX link word.
        if self.cw_active() {
            self.cw_controls(false)[0]
        } else if self.automatic() {
            0xC1FA
        } else {
            0xC1FE
        }
    }

    /// Consume the audio checkpoint once. Preparation time already elapsed
    /// during RF work is never counted again. If necessary, the existing
    /// Delay phase waits only until the original absolute audio deadline.
    ///
    /// The RF setup checkpoints precede this operation. Reaching the audio
    /// deadline alone cannot select On, enable PA, or authorize an element.
    #[cfg(feature = "cw")]
    const fn finish_cw_audio(&mut self, now: u32) {
        self.step += 1;
        if !reached(now, self.cw_audio_at) {
            self.wake_at = self.cw_audio_at;
            self.phase = Phase::Delay;
        }
    }

    /// Complete CW mark or space control expectation. REG_30 bit three
    /// admits internal PA gain; bit two remains clear in both states.
    /// Space also removes calibrated PA control and parks board outputs.
    ///
    /// The reference CW keyer removes internal PA gain and external
    /// PA-enable between elements. Its additional AF-DAC bit belongs to
    /// local sidetone and is not introduced into this muted TX path.
    const fn cw_controls(&self, down: bool) -> [u16; 5] {
        let audio = self.cw_audio_active();
        [
            (if down { 0xC1FA } else { 0xC1F2 })
                | if audio { 0x0200 } else { 0 },
            if down { self.profile.gpio } else { 0x9000 },
            if down { self.profile.pa } else { 0 },
            0x9D1F,
            if audio { 0x6242 } else { 0x6042 },
        ]
    }

    #[cfg(feature = "cw")]
    pub(crate) fn cw_ready(&self) -> bool {
        self.cw_active()
            && (matches!(self.phase, Phase::On)
                || (matches!(self.phase, Phase::Practice)
                    && reached(self.now, self.wake_at)))
    }

    /// Session readiness stays true during receive windows so the keyer can
    /// request another mark. Electronic mark timing additionally waits for
    /// this checked TX destination; practice retains its ordinary readiness.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_mark_ready(&self) -> bool {
        self.cw_ready() && (!self.breakin.active() || self.breakin.tx_ready())
    }

    /// Observed turnaround allowance for an already admitted CW session.
    /// This is an electronic-keyer scheduling lead, not a hardware minimum.
    /// It changes neither mark duration nor the complete-session timeout.
    #[cfg(feature = "cw")]
    pub(crate) fn cw_prepare_ticks(&self) -> u32 {
        if self.breakin.active() { self.breakin.advance() } else { 0 }
    }

    const fn fsk_status(&self) -> u8 {
        #[cfg(feature = "fsk")]
        {
            self.fsk_status
        }
        #[cfg(not(feature = "fsk"))]
        {
            0
        }
    }

    const fn fsk_unchanged(&self, _context: &Context) -> bool {
        #[cfg(feature = "fsk")]
        {
            if let Some(crate::link::Packet::Ack {
                sender, destination, sequence,
            }) = self.automatic_ack {
                return matches!(
                    _context.fsk_ack,
                    Some(crate::link::Packet::Ack {
                        sender: peer, destination: target, sequence: number,
                    }) if sender == peer && destination == target && sequence == number,
                );
            }

            self.fsk_status == _context.fsk_status
                && match (self.fsk_message, _context.fsk_message) {
                    (None, None) => true,
                    (Some(left), Some(right)) => left.same(&right),
                    _ => false,
                }
        }
        #[cfg(not(feature = "fsk"))]
        {
            true
        }
    }

    /// This engaged phase owns no TX hardware and requires receiver pumping.
    pub(crate) const fn acquiring(&self) -> bool {
        matches!(self.phase, Phase::Acquire)
    }

    /// True excludes refusal audio and identifies only the automatic voice
    /// attempt. Physical PTT remains unchanged in Context and Release.
    pub(crate) fn vox_active(&self) -> bool {
        !cfg!(feature = "cw") && self.vox_attempt && self.engaged() && !matches!(self.phase, Phase::Alert)
    }

    pub(crate) fn vox_sample(&self, now: u32) -> Option<crate::vox::Sample> {
        if !cfg!(feature = "cw") && self.vox_attempt && matches!(self.phase, Phase::On) {
            self.vox.sample(now)
        } else {
            None
        }
    }

    pub(crate) fn take_vox_finished(&mut self) -> bool {
        if cfg!(feature = "cw") { return false; }
        let finished = self.vox_finished;
        self.vox_finished = false;
        finished
    }

    pub(crate) fn voice_active(&self) -> bool {
        matches!(self.phase, Phase::On) && !self.cw_active() && !self.automatic()
    }


    pub(crate) fn engaged(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Storage and recovery actions invalidate any held PTT history.
    /// This is not a persistent or user-armed transmit permit.
    pub(crate) fn disarm(&mut self) {
        self.release.lock();
    }

    pub(crate) fn controls(&self) -> Option<[u16; 5]> {
        if self.acquiring() {
            None
        } else if matches!(self.phase, Phase::Alert | Phase::Permit) {
            self.local.controls()
        } else if self.engaged() {
            Some(self.expected)
        } else {
            None
        }
    }

    #[inline(never)]
    pub(crate) fn view(&self) -> View {
        let now = mcu::tick::snapshot();
        let error = self.notice.current(now, self.error);
        View {
            active: self.engaged(),
            slope: cfg!(feature = "fam") && self.receive_am && self.slope.enabled,
            blocked: self.release.blocked,
            status: match self.phase {
                #[cfg(feature = "cw")]
                Phase::Practice => if reached(self.now, self.wake_at) {
                    b"LOCAL PRACTICE"
                } else {
                    b"LOCAL SETUP"
                },
                Phase::Acquire => b"CHECK SELECTED RX",
                Phase::Alert => b"TX ERROR",
                Phase::Permit => b"LOCAL PERMIT",
                Phase::Setup | Phase::Delay => b"TX SETUP",
                Phase::On if self.cw_active() => {
                    if self.expected[2] != 0 { b"CW MARK" } else { b"CW SPACE" }
                }
                Phase::On if self.receive_am => b"SC-FM TRANSMIT",
                Phase::On if !cfg!(feature = "cw") && self.vox_attempt => b"VOX TRANSMITTING",
                Phase::On => b"TRANSMITTING",
                Phase::Tail => if self.fsk_status() != 0 {
                    b"FSK PACKET"
                } else {
                    b"ROGER TAIL"
                },
                Phase::Idle if self.release.blocked => b"RELEASE PTT",
                Phase::Idle if error != 0 => b"TX REFUSED",
                Phase::Idle => b"PTT READY",
            },
            frequency: if cfg!(feature = "fam") && self.receive_am && self.slope.enabled {
                self.receive_frequency
            } else {
                self.frequency
            },
            narrow: self.narrow,
            elapsed: if self.engaged() {
                self.now.wrapping_sub(self.started)
            } else { 0 },
            bias: self.profile.bias as u32,
            error,
            carrier: self.engaged() && self.expected[2] != 0,
            #[cfg(feature = "cw")]
            keyed: self.mark_state(),
            budget: self.budget,
            ham: self.ham,
            power: self.power,
            blink: error != 0 && now / 250 & 1 == 0,
        }
    }

    const fn battery(profile: Profile, reading: Reading) -> Result<(), u32> {
        let raw = match reading.raw {
            Some(raw) if reading.error == 0 => raw,
            _ => return Err(0x7004),
        };
        if raw < profile.battery_min || raw > profile.battery_max {
            Err(0x7004)
        } else {
            Ok(())
        }
    }

    /// Evaluates one foreground snapshot without register access.
    /// PTT edge and release qualification are checked by pump before this
    /// predicate runs. Calibration and battery freshness are supplied by
    /// their owners; this predicate does not acquire or refresh either.
    const fn admit(context: &Context) -> Result<Profile, u32> {
        Self::evaluate(context, true)
    }

    /// Early acquisition admission checks the complete TX envelope without
    /// mistaking the other VFO's current receiver view for selected RX.
    /// Final admission additionally requires the selected settled receiver.
    #[inline(never)]
    const fn evaluate(context: &Context, check_receiver: bool) -> Result<Profile, u32> {
        if context.request.am && (!cfg!(feature = "fam") || !context.slope.enabled) {
            return Err(0x700F);
        }
        if !crate::app::tx_options::valid_ticks(context.tot) {
            return Err(0x700A);
        }

        if !crate::tx_signal::valid(context.tx_signal as u32) {
            return Err(0x700D);
        }

        let frequency = match context.transmit {
            Ok(frequency) => frequency,
            Err(error) => return Err(error),
        };

        #[cfg(feature = "cw")]
        if context.cw_pitch != 0 {
            if !crate::cw::valid_pitch(context.cw_pitch)
                || crate::cw::rx_frequency(
                    context.request.frequency, context.cw_pitch,
                ).is_none()
                || context.request.am || !context.request.narrow
                || context.request.rx_signal != 0 || context.tx_signal != 0
                || frequency != context.request.frequency
                || context.cw_breakin >= 8
                || crate::cw_tail::Settings::decode(context.cw_tail as u32).is_none()
            {
                return Err(0x7A01);
            }

            #[cfg(feature = "fsk")]
            if context.fsk_status != 0 {
                return Err(0x7A03);
            }
        }

        #[cfg(feature = "fsk")]
        if context.fsk_status != 0
            && (context.fsk_status > crate::fsk::STATUS_COUNT
                || context.request.am || !context.request.narrow
                || context.request.rx_signal != 0 || context.tx_signal != 0
                || frequency != context.request.frequency)
        {
            return Err(0x7801);
        }

        if context.request.am {
            let expected = match context.slope.frequency(context.request.frequency) {
                Ok(frequency) => frequency,
                Err(error) => return Err(error),
            };
            if frequency != expected || context.tx_signal != 0 {
                return Err(0x7013);
            }

            // Shifting cannot be used to enter an allocation from a nominal
            // frequency that the same policy would reject.
            match tx_policy::check(
                context.request.frequency,
                context.tx_narrow(),
                context.ham,
                context.power,
            ) {
                Ok(()) => {}
                Err(error) => return Err(error),
            }
        }

        match tx_policy::check(
            frequency,
            context.tx_narrow(),
            context.ham,
            context.power,
        ) {
            Ok(()) => {}
            Err(error) => return Err(error),
        }

        if !context.allowed || !context.request.enabled
            || context.request.monitor || context.request.squelch == 0
        {
            return Err(0x7001);
        }
        if check_receiver {
            if !context.receiver_settled() {
                return Err(0x7003);
            }
            if context.receiver.signal {
                return Err(0x7005);
            }
        }

        let profile = match context.calibration {
            Ok(profile) => profile,
            Err(error) => return Err(error),
        };


        match Self::battery(profile, context.battery) {
            Ok(()) => Ok(profile),
            Err(error) => Err(error),
        }
    }

    /// Both sides of a split attempt are immutable. The receiver view
    /// remains an RX admission snapshot, never a substitute for TX tuning.
    #[inline(never)]
    const fn unchanged(&self, context: &Context) -> bool {
        context.allowed && context.request.enabled
            && context.request.am == self.receive_am
            && (!self.receive_am || context.slope.word() == self.slope.word())
            && context.request.frequency == self.receive_frequency
            && context.request.rx_signal == self.receive_signal
            && context.request.narrow == self.receive_narrow
            && context.tx_narrow() == self.narrow
            && !context.request.monitor
            && context.ham == self.ham
            && context.power == self.power
            && context.tot == self.budget
            && context.tx_signal == self.tx_signal
            && self.fsk_unchanged(context)
            && self.cw_unchanged(context)
            && (cfg!(feature = "cw") || !self.vox_attempt || context.vox_threshold == self.vox_threshold)
            && context.voice.word() == self.voice.word()
            && (self.training()
                || matches!(context.transmit, Ok(frequency) if frequency == self.frequency))
    }

    fn observe_audio(&mut self, talking: bool) {
        self.audio_expected = if talking && !self.cw_active() && !self.automatic() {
            0x3B18
        } else {
            0xBB18
        };
        self.audio_actual = read(0x50);
    }

    #[inline(never)]
    fn check_tuning(&self) -> Result<(), u32> {
        let frequency = read(0x38) as u32 | ((read(0x39) as u32) << 16);
        if frequency != self.frequency / 10 { return Err(0x7238); }
        if read(0x43) != if self.narrow { 0x4048 } else { 0x3028 } {
            return Err(0x7243);
        }
        if read(0x3B) != self.profile.crystal { return Err(0x723B); }
        if read(0x40) != 0x3516 { return Err(0x7240); }
        if read(0x7D) != self.microphone_config() { return Err(0x727D); }
        let tone_control = self.tone_control();
        if read(0x31) & 15 != (
            (if self.vox.active() { 4 } else { 0 }) | self.processing.control()
        )
            || (!matches!(self.phase, Phase::Tail) && read(0x70) != tone_control)
        {
            return Err(0x7231);
        }
        if self.receive_am {
            super::demod::check(false, self.vox.active())?;
        }

        if self.cw_active() {
            // REG_50 receives the established BB18 mute command during
            // preparation and cleanup. Its readback has no verified mirror
            // contract and cannot authorize or reject CW transmission.
            // observe_audio retains that readback as diagnostic data only.
            //
            // MIC ADC exclusion remains checked through the exact REG_30
            // control tuple. REG_70 and REG_51 checks above exclude tone
            // generators and subaudio. Retain the remaining FSK checks.
            //
            // Report a mismatch as 7Brrvvvv before RF-off cleanup changes
            // the observed state. Register retention is not an independent
            // measurement of RF modulation or carrier extinction.
            for &(register, expected) in &[
                (0x58u8, 0u16),
                (0x59, 0),
                (0x3F, 0),
            ] {
                let actual = read(register);
                if actual != expected {
                    return Err(
                        ((0x7B00u32 | register as u32) << 16)
                            | actual as u32,
                    );
                }
            }
        }

        #[cfg(feature = "cw")]
        self.sidetone.check()?;

        self.processing.check()?;

        if !cfg!(feature = "cw") && self.vox_attempt {
            self.vox.check()?;
            let mask = if self.vox.active() { super::vox::MASK } else { 0 };
            if read(0x3F) != mask {
                return Err(0x7C3F);
            }
        }

        if read(0x51) != super::subtone::tx_control(self.tx_signal) {
            return Err(0x7251);
        }
        Ok(())
    }

    #[inline(never)]
    fn check_setup(&mut self) -> Result<(), u32> {
        let af = if self.cw_audio_active() { 0x6242 } else { 0x6042 };
        if self.expected != [self.tx_links(), 0x9000, 0, 0x9D1F, af] {
            return Err(0x7201);
        }
        let _ = verify_transmit(self.expected)?;
        self.check_tuning()?;
        self.observe_audio(false);
        self.checks = self.checks.saturating_add(1);
        Ok(())
    }

    #[inline(never)]
    fn check(&mut self, talking: bool) -> Result<(), u32> {
        let actual = verify_transmit(self.expected)?;
        let wanted = if self.cw_active() {
            self.cw_controls(self.expected[2] != 0)
        } else {
            [
                self.expected[0], self.profile.gpio, self.profile.pa,
                0x9D1F, 0x6042,
            ]
        };

        if self.expected != wanted
            || actual[0] & 0x7FFF != wanted[0] & 0x7FFF
            || actual[1] != wanted[1]
            || actual[2] != wanted[2]
        {
            return Err(0x7201);
        }
        self.check_tuning()?;
        self.observe_audio(talking);
        self.checks = self.checks.saturating_add(1);
        Ok(())
    }

    fn begin(&mut self, now: u32, receiver: &mut Receiver) {
        let acquired = self.acquiring();
        #[cfg(feature = "cw")]
        {
            self.breakin.clear();
            if self.cw_active() {
                if let Some(wait) = crate::cw_controls::breakin_ticks(self.cw_breakin) {
                    let shifted = match crate::cw::rx_frequency(
                        self.receive_frequency, self.cw_pitch,
                    ) {
                        Some(value) => value,
                        None => {
                            self.reject(now, 0x7A01, receiver);
                            return;
                        }
                    };
                    if let Err(error) = self.breakin.capture(
                        self.receive_frequency / 10, shifted / 10, wait,
                        self.cw_tail,
                    ) {
                        // TX setup has not captured cleanup registers or
                        // armed its backstop yet. Use the refusal path.
                        self.reject(now, error, receiver);
                        return;
                    }
                }
            }
        }
        receiver.suspend_for_tx();
        if let Err(error) = verify() { startup::stop(error); }

        self.original_audio = [read(0x7D), read(0x7E), read(0x31)];
        self.expected = [0, 0x9000, 0, 0x9D1F, 0x6042];
        self.step = 0;
        if !acquired {
            self.started = now;
            self.deadline = now.wrapping_add(self.budget);
        }
        self.phase = Phase::Setup;
        self.error = 0; self.notice.report(self.now, 0);
        self.starts = self.starts.saturating_add(1);
        self.release.lock();
        mcu::tx_limit::arm(self.started, self.budget);

        #[cfg(feature = "cw")]
        if self.cw_active() {
            // Admission, parked-RF verification, captured restoration state,
            // and the fixed attempt cutoff already exist. Start only silent
            // local audio; RF preparation then overlaps its settling time.
            if let Err(error) = self.start_cw_audio() {
                self.stop(error);
                return;
            }
        }

        if self.sounds.permit != 0 {
            match self.local.start(now, crate::app::sound::permit(self.sounds.permit)) {
                Ok(()) => self.phase = Phase::Permit,
                Err(error) => self.stop(error),
            }
        }
    }

    /// Acquisition has not armed the TX timer or captured TX audio registers.
    /// Its cancellation must not use the ordinary TX cleanup with stale
    /// original_audio values. Park and verify RX, then consume PTT history.
    fn cancel_acquisition(&mut self, error: u32, receiver: &mut Receiver) {
        self.vox_finished |= !cfg!(feature = "cw") && self.vox_attempt;
        #[cfg(feature = "fsk")]
        if let Some(packet) = self.automatic_ack.take() {
            self.ack_result = Some((packet, false));
            self.ack_completed = false;
        }

        receiver.suspend_for_tx();
        self.expected = match verify() {
            Ok(actual) => actual,
            Err(failure) => startup::stop(failure),
        };
        self.phase = Phase::Idle;
        self.release.lock();
        self.error = error;
        self.notice.report(self.now, error);
    }

    #[inline(never)]
    fn pump_acquisition(&mut self, now: u32, context: &Context, receiver: &mut Receiver) {
        if self.automatic() {
            if context.ptt || context.raw_ptt {
                self.cancel_acquisition(0x7503, receiver);
                return;
            }
        } else if !cfg!(feature = "cw") && self.vox_attempt {
            if !context.vox_request || context.ptt || context.raw_ptt {
                self.cancel_acquisition(0, receiver);
                return;
            }
        } else if !context.raw_ptt {
            self.cancel_acquisition(0, receiver);
            return;
        }
        if !self.unchanged(context) {
            self.cancel_acquisition(0x7007, receiver);
            return;
        }

        match Self::evaluate(context, false) {
            Ok(profile) if profile == self.profile => {}
            Ok(_) => {
                self.reject(now, 0x7008, receiver);
                return;
            }
            Err(error) => {
                self.reject(now, error, receiver);
                return;
            }
        }

        let settled = context.receiver_settled();

        let qualification = if !cfg!(feature = "cw") && self.vox_attempt {
            self.acquisition.poll_vox(now, settled, context.receiver.signal)
        } else {
            #[cfg(feature = "cw")]
            {
                if self.cw_active() && !context.dual_watch {
                    self.acquisition.poll_cw(
                        now, settled, context.receiver.signal,
                    )
                } else {
                    self.acquisition.poll(
                        now, settled,
                        context.receiver.signal, context.receiver.sample_number,
                    )
                }
            }

            #[cfg(not(feature = "cw"))]
            {
                self.acquisition.poll(
                    now, settled,
                    context.receiver.signal, context.receiver.sample_number,
                )
            }
        };

        match qualification {
            Ok(false) => {}
            Err(error) => self.reject(now, error, receiver),
            Ok(true) => match Self::admit(context) {
                Ok(_) => self.begin(now, receiver),
                Err(error) => self.reject(now, error, receiver),
            },
        }
    }

    fn reject(&mut self, now: u32, error: u32, receiver: &mut Receiver) {
        self.vox_finished |= !cfg!(feature = "cw") && self.vox_attempt;
        if self.automatic() {
            self.cancel_acquisition(error, receiver);
            return;
        }

        receiver.suspend_for_tx();
        self.expected = match verify() {
            Ok(actual) => actual,
            Err(failure) => startup::stop(failure),
        };
        self.release.lock();
        self.error = error;
        self.notice.report(self.now, error);
        self.started = now;

        match self.local.start(now, crate::app::sound::tx_error()) {
            Ok(()) => self.phase = Phase::Alert,
            Err(failure) => startup::stop(failure),
        }
    }

    fn stop(&mut self, error: u32) {
        write(0x36, 0);
        write(0x33, 0x9000);
        write(0x30, 0);
        unsafe { mcu::write(mcu::GPIO_A + 0x18, 1 << 24) };


        self.processing.stop();
        self.vox.stop();
        if !cfg!(feature = "cw") && self.vox_attempt {
            write(0x3F, 0);
            self.vox_finished = true;
        }
        #[cfg(feature = "cw")]
        {
            self.sidetone.cancel();
            self.breakin.clear();
        }
        self.local.cancel();
        self.roger.cancel();
        write(0x51, 0);
        write(0x70, 0);
        write(0x59, 0);
        write(0x58, 0);
        write(0x47, 0x6042);
        write(0x50, 0xBB18);
        write(0x37, 0x9D1F);
        write(0x7D, self.original_audio[0]);
        write(0x7E, self.original_audio[1]);
        write(0x31, self.original_audio[2]);

        self.expected = match verify() {
            Ok(actual) => actual,
            Err(failure) => startup::stop(failure),
        };
        mcu::tx_limit::clear();

        #[cfg(feature = "fsk")]
        if let Some(packet) = self.automatic_ack.take() {
            self.ack_result = Some((packet, self.ack_completed && error == 0));
            self.ack_completed = false;
        }

        self.phase = Phase::Idle;
        self.release.lock();
        self.error = error;
        self.notice.report(self.now, error);
    }

    const fn sequence(&self) -> Step {
        let frequency = self.frequency / 10;
        match self.step {
            0 => Step::Write(0x36, 0),
            1 => Step::Write(0x33, 0x9000),
            2 => Step::Write(
                0x30, if self.cw_audio_active() { 0x0302 } else { 0 },
            ),
            3 => Step::Write(
                0x47, if self.cw_audio_active() { 0x6242 } else { 0x6042 },
            ),
            4 => Step::Write(0x50, 0xBB18),
            5 => Step::Write(0x38, frequency as u16),
            6 => Step::Write(0x39, (frequency >> 16) as u16),
            7 => Step::Write(0x43, if self.narrow { 0x4048 } else { 0x3028 }),
            8 => Step::Write(0x3B, self.profile.crystal),
            9 => Step::Microphone,
            10 => Step::Write(0x7E, (self.original_audio[1] & !(7 << 3)) | (5 << 3)),
            11 => Step::Write(
                0x31, (self.original_audio[2] & !15) | self.processing.control(),
            ),
            12 => Step::Write(0x2B, 0),
            13 => Step::Write(0x24, 0),
            14 => Step::Write(0x70, self.tone_control()),
            15 => Step::Subtone,
            16 => Step::Write(0x52, 0x028F),
            17 => Step::Write(0x37, 0x9D1F),
            18 if self.receive_am => Step::Fm,
            18 => Step::Write(0x40, 0x3516),
            19 => Step::Write(0x30, self.tx_links()),
            20 => Step::Wait(10),
            21 => Step::Write(0x50, 0xBB18),
            22 => Step::Wait(2),
            23 => Step::Controls,
            24 => Step::Write(
                0x33, if self.cw_active() { 0x9000 } else { self.profile.gpio },
            ),
            25 => Step::Wait(5),
            26 => Step::Write(0x36, if self.cw_active() { 0 } else { self.profile.pa }),
            27 => Step::Wait(10),
            28 => Step::Check,
            29 => Step::Write(
                0x50,
                if self.cw_active() || self.automatic() { 0xBB18 } else { 0x3B18 },
            ),
            30 => Step::Wait(2),
            #[cfg(feature = "cw")]
            31 if self.cw_active() => Step::CwAudio,
            _ => Step::Finish,
        }
    }

    fn configure(&mut self, register: u8, value: u16) -> Result<(), u32> {
        let permitted = match register {
            0x30 => matches!(value, 0 | 0xC1FE)
                || (self.cw_active() && matches!(value, 0xC1F2 | 0xC1FA))
                || (self.cw_active() && self.cw_audio_active()
                    && matches!(value, 0x0302 | 0xC3F2 | 0xC3FA))
                || (value == 0xC1FA
                    && (matches!(self.phase, Phase::Tail) || self.automatic())),
            0x33 => value == 0x9000 || value == self.profile.gpio,
            0x36 => value == 0 || value == self.profile.pa,
            0x37 => value == 0x9D1F,
            0x47 => value == 0x6042
                || (self.cw_active() && self.cw_audio_active() && value == 0x6242),
            _ => true,
        };
        if !permitted { return Err(0x7400 + register as u32); }

        // A prepared CW edge has its own complete space-state check before
        // enabling the board PA. It is not another initial setup sequence.
        if register == 0x33 && value != 0x9000
            && !(self.cw_active() && matches!(self.phase, Phase::On))
        {
            self.check_setup()?;
        }
        if register == 0x30 && value == 0xC1FA {
            self.processing.tail();
        }
        write(register, value);
        match register {
            0x30 => self.expected[0] = value,
            0x33 => self.expected[1] = value,
            0x36 => self.expected[2] = value,
            0x37 => self.expected[3] = value,
            0x47 => self.expected[4] = value,
            _ => {}
        }
        Ok(())
    }

    /// Key one prepared CW session without restarting the synthesizer.
    /// A mark enables internal PA gain, selects the calibrated board path,
    /// then applies the admitted PA code. Space removes that code first,
    /// parks external PA-enable, then disables internal PA gain.
    ///
    /// Every transition starts from the exact opposite complete tuple.
    /// Readback checks the starting tuple and the completed destination.
    /// The watchdog guard receives only the completed foreground state.
    /// A failed write or readback uses ordinary complete-session shutdown.
    ///
    /// REG_30 MIC ADC remains disabled. Frequency, calibration, modulation
    /// mute, release qualification, and the original TOT are unchanged.
    /// No tone generator, sidetone, blocking delay, or envelope ramp is used.
    /// Register retention does not establish RF extinction or key-click level.
    #[cfg(feature = "cw")]
    fn cw_key(&mut self, down: bool) -> Result<(), u32> {
        let wanted = self.cw_controls(down);
        if self.expected == wanted {
            return Ok(());
        }

        if self.expected != self.cw_controls(!down) {
            return Err(0x7201);
        }
        let _ = verify_transmit(self.expected)?;

        if down {
            self.configure(0x30, wanted[0])?;
            self.configure(0x33, wanted[1])?;
            self.configure(0x36, wanted[2])?;
        } else {
            self.configure(0x36, 0)?;
            self.configure(0x33, 0x9000)?;
            self.configure(0x30, wanted[0])?;
        }

        if !down && self.breakin.active() {
            // RF controls already hold the complete PA-off space tuple.
            // Retain local oscillator frequency until the output gain
            // reaches zero; an immediate frequency-zero write can click.
            self.sidetone.defer_release();
        } else {
            self.sidetone.key(down);
        }
        let _ = verify_transmit(wanted)?;
        Ok(())
    }

    /// Service one transmitter-owned receive window. Existing CW RF edges
    /// are used only from prepared TX. An RX tuple never reaches cw_key or
    /// the normal TX-frequency check. TX restoration must pass that complete
    /// checkpoint before the next foreground mark can start.
    #[cfg(feature = "cw")]
    fn pump_breakin(&mut self, now: u32, down: bool, prepare: bool) {
        let was_tx = self.breakin.tx_ready();
        if was_tx {
            if let Err(error) = self.cw_key(down) {
                self.stop(error);
                return;
            }
        }

        // Advance the existing output fade before any destination check.
        // pump services it too, but its deadline prevents another step
        // at this timestamp. Oscillator release precedes RX programming.
        self.breakin.service_audio(now);
        if self.sidetone.release_pending()
            && self.breakin.audio_release_ready(now)
        {
            self.sidetone.key(false);
        }
        self.breakin.space_tone(self.sidetone.expected_word());

        match self.breakin.pump(now, prepare || down, self.expected) {
            Ok(expected) => self.expected = expected,
            Err(error) => {
                self.stop(error);
                return;
            }
        }

        if self.breakin.tx_ready() && !was_tx {
            if let Err(error) = self.check(false) {
                self.stop(error);
                return;
            }
            let after = mcu::tick::snapshot();
            self.breakin.note_tx_ready(after);
            self.next_check = after.wrapping_add(100);
        } else if reached(now, self.next_check) {
            let result = if self.breakin.tx_ready() {
                self.check(true)
            } else {
                self.breakin.check(self.expected)
                    .and_then(|_| self.sidetone.check())
            };
            if let Err(error) = result {
                self.stop(error);
                return;
            }
            self.next_check = now.wrapping_add(100);
        }
    }

    /// Prepare the same silent local path for RF CW and practice.
    /// Entry requires the exact parked tuple after receiver suspension.
    /// Restoration state and the session cutoff belong to the caller.
    ///
    /// Tone1 starts at zero frequency. Only local AF DAC and DSP links are
    /// enabled here; synthesizer, RX/TX RF links, MIC ADC, PA gain, external
    /// PA gate, and PA drive remain off. Speaker enable follows complete
    /// local-tuple verification.
    ///
    /// The seventy-tick deadline starts after speaker enable. RF setup may
    /// proceed concurrently, retaining AF DAC, AF_ALAM, and the silent tone
    /// generator. The first mark waits for both the original audio deadline
    /// and all ordinary RF setup checkpoints.
    #[cfg(feature = "cw")]
    fn start_cw_audio(&mut self) -> Result<(), u32> {
        if self.expected != [0, 0x9000, 0, 0x9D1F, 0x6042] {
            return Err(0x7201);
        }
        let _ = verify_transmit(self.expected)?;
        write(0x50, 0xBB18);
        self.sidetone.start(self.cw_pitch, self.cw_volume)?;

        self.configure(0x47, 0x6242)?;
        self.configure(0x30, 0x0302)?;
        let _ = verify_transmit(self.expected)?;
        unsafe { mcu::write(mcu::GPIO_A + 0x18, 1 << 8) };
        self.cw_audio_at = mcu::tick::snapshot().wrapping_add(70);
        Ok(())
    }

    /// CW may execute at most four consecutive sequence actions per call.
    /// Voice retains one action per foreground pass. Every Write, Subtone,
    /// Controls, Check, and local-audio action keeps its sequence position.
    ///
    /// A Wait always yields, even if its duration is short. Its CW deadline
    /// starts after preceding register work, using a fresh tick snapshot.
    /// Hardware waits are neither consumed by earlier writes nor shortened.
    ///
    /// CW also yields after two observed ticks of setup work. The bound is
    /// checked between actions: one transport operation or register check
    /// cannot be interrupted midway by this executor. Lost SysTick periods
    /// under transaction masking are not measured by this limit.
    ///
    /// Policy, battery, calibration, request identity, and cancellation are
    /// checked by pump before entry. The fixed attempt deadline is checked
    /// again before each CW action. No batch renews release qualification,
    /// the attempt budget, or its independent SysTick cutoff.
    #[cfg(feature = "cw")]
    fn begin_practice(
        &mut self, now: u32, context: &Context, receiver: &mut Receiver,
    ) -> Result<(), u32> {
        // Practice has no TX frequency, PA calibration, or allocation
        // admission. It still requires an eligible UI owner, valid pitch,
        // an explicit live request, and a checked bounded session duration.
        if !context.allowed || !context.request.enabled
            || context.request.am || context.request.monitor
            || !crate::cw::valid_pitch(context.cw_pitch)
            || !crate::app::tx_options::valid_ticks(context.tot)
        {
            return Err(0x7001);
        }

        receiver.suspend_for_tx();
        let _ = verify()?;
        self.original_audio = [read(0x7D), read(0x7E), read(0x31)];
        self.profile = Profile::empty();
        self.breakin.clear();
        self.tx_signal = context.tx_signal;
        self.budget = context.tot;
        self.started = now;
        self.deadline = now.wrapping_add(self.budget);
        self.next_check = now.wrapping_add(100);
        self.release.lock();
        self.phase = Phase::Practice;
        self.error = 0;
        self.notice.report(now, 0);
        mcu::tx_limit::arm(now, self.budget);

        // Practice keeps this local-only tuple throughout the session.
        // The shared helper cannot enable any RF path.
        self.expected = [0, 0x9000, 0, 0x9D1F, 0x6042];
        self.start_cw_audio()?;
        self.wake_at = self.cw_audio_at;
        Ok(())
    }

    /// Practice shares cancellation and complete-session limits, not TX
    /// admission. Exact local controls and Tone1 retention are checked
    /// periodically. No branch can switch this phase into an RF phase.
    #[cfg(feature = "cw")]
    fn pump_practice(&mut self, now: u32, context: &Context) {
        if !self.unchanged(context) {
            self.stop(0x7007);
            return;
        }
        if reached(now, self.deadline) {
            self.timeouts = self.timeouts.saturating_add(1);
            self.stop(0x7006);
            return;
        }
        if !context.raw_ptt {
            let completed = context.cw_complete && !self.sidetone.down();
            self.stop(0);
            if completed {
                self.release.accept_cw_release(now);
            }
            return;
        }

        if reached(now, self.wake_at) {
            self.sidetone.key(context.cw_down);
        }

        if reached(now, self.next_check) {
            let result = verify_transmit(self.expected)
                .and_then(|_| self.sidetone.check());
            if let Err(error) = result {
                self.stop(error);
            } else {
                self.next_check = now.wrapping_add(100);
            }
        }
    }

    #[inline(never)]
    fn pump_setup(&mut self, now: u32) {
        let cw = self.cw_active();
        let limit = if cw { 4 } else { 1 };
        let started = if cw { mcu::tick::snapshot() } else { now };

        for _ in 0..limit {
            if cw && reached(mcu::tick::snapshot(), self.deadline) {
                self.timeouts = self.timeouts.saturating_add(1);
                self.stop(0x7006);
                return;
            }

            match self.sequence() {
                Step::Microphone => {
                    let result = self.configure(0x7D, self.microphone_config())
                        .and_then(|_| {
                            if !self.cw_active() && !self.automatic() {
                                self.processing.prepare(self.voice)
                            } else {
                                Ok(())
                            }
                        });
                    match result {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    }
                }
                #[cfg(feature = "cw")]
                Step::CwAudio => {
                    self.finish_cw_audio(mcu::tick::snapshot());
                }
                Step::Fm => {
                    super::demod::configure(false);
                    match self.configure(0x40, 0x3516) {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    }
                }
                Step::Subtone => {
                    // Thresholds precede comparator enable. Existing RF
                    // setup waits and PA admission positions are unchanged.
                    let result = super::subtone::configure_tx(self.tx_signal)
                        .and_then(|_| {
                            if !cfg!(feature = "cw") && self.vox_attempt {
                                write(0x3F, super::vox::MASK);
                                self.vox.start(now, self.vox_threshold)
                            } else {
                                Ok(())
                            }
                        });
                    match result {
                        Ok(()) => {
                            if self.cw_active() {
                                write(0x58, 0);
                                write(0x59, 0);
                                write(0x3F, 0);
                            }
                            self.step += 1;
                        }
                        Err(error) => self.stop(error),
                    }
                }
                Step::Write(register, value) => {
                    match self.configure(register, value) {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    }
                }
                Step::Wait(ticks) => {
                    let after = if cw { mcu::tick::snapshot() } else { now };
                    self.step += 1;
                    self.wake_at = after.wrapping_add(ticks);
                    self.phase = Phase::Delay;
                    return;
                }
                Step::Controls => match self.check_setup() {
                    Ok(()) => self.step += 1,
                    Err(error) => self.stop(error),
                },
                Step::Check => match self.check(false) {
                    Ok(()) => self.step += 1,
                    Err(error) => self.stop(error),
                },
                Step::Finish => {
                    match self.check(true) {
                        Ok(()) => {
                            let after = if cw {
                                mcu::tick::snapshot()
                            } else {
                                now
                            };
                            // The next foreground pass requires a TX-owned
                            // detector sample. Produce its first observation
                            // before publishing On; an unknown event latch
                            // is valid input and cannot renew voice hold.
                            if !cfg!(feature = "cw") && self.vox_attempt {
                                self.vox.pump(after);
                            }
                            #[cfg(feature = "cw")]
                            if self.breakin.active() {
                                if let Err(error) = self.breakin.prepare(self.expected) {
                                    self.stop(error);
                                    return;
                                }
                            }
                            self.phase = Phase::On;
                            self.next_check = after.wrapping_add(100);
                        }
                        Err(error) => self.stop(error),
                    }
                    return;
                }
            }

            if !matches!(self.phase, Phase::Setup) {
                return;
            }
            if cw && mcu::tick::snapshot().wrapping_sub(started) >= 2 {
                return;
            }
        }
    }

    /// Consider one queued response only while idle and physically released.
    /// The normal UI scope has a separate automatic-response permission;
    /// this never makes manual PTT legal on Inbox or Settings.
    ///
    /// An exact settled silent receiver precedes the same bounded advancing-
    /// sample quiet acquisition used by DW. Policy, simplex NFM restrictions,
    /// calibration, battery, and TOT are checked before acquisition and again
    /// before setup. Acquisition has no TX hardware ownership.
    ///
    /// Setup retains every existing register checkpoint and wait. MIC ADC is
    /// excluded before PA admission and voice modulation is never unmuted.
    /// The ordinary packet owner then performs the unchanged FIFO sequence.
    /// Physical PTT, lost scope, changed profile, or an error cancels the
    /// response; no response retries or alert-tone playback are scheduled.
    #[cfg(feature = "fsk")]
    fn try_ack(&mut self, now: u32, context: &mut Context) {
        let packet = match context.fsk_ack {
            Some(crate::link::Packet::Ack { sender, destination, sequence })
                if sender != 0 && destination != 0 && sequence != 0 =>
            {
                crate::link::Packet::Ack { sender, destination, sequence }
            }
            _ => return,
        };

        if !context.fsk_ack_allowed || !context.request.enabled
            || context.ptt || context.raw_ptt
            || !self.release.ready || self.release.blocked
            || !context.receiver_settled() || context.receiver.signal
            || context.receiver.error != 0
        {
            return;
        }

        // Automatic scope and physical release were checked above. The
        // caller owns this detached context and performs no manual dispatch
        // after this method, so admission needs no second aggregate.
        //
        // The sentinel selects existing NFM/simplex packet restrictions.
        // It is never encoded as a status packet for this automatic attempt.
        context.allowed = true;
        context.fsk_status = 1;
        context.fsk_message = None;
        let profile = match Self::evaluate(context, false) {
            Ok(profile) => profile,
            Err(_) => {
                self.ack_result = Some((packet, false));
                return;
            }
        };

        #[cfg(feature = "cw")]
        {
            self.cw_pitch = context.cw_pitch;
            self.cw_qsk = context.cw_qsk;
            self.cw_volume = context.cw_volume;
            self.cw_breakin = context.cw_breakin;
            self.cw_tail = context.cw_tail;
        }

        self.vox_attempt = false;
        self.voice = context.voice;
        self.profile = profile;
        self.receive_frequency = context.request.frequency;
        self.receive_signal = context.request.rx_signal;
        self.receive_am = context.request.am;
        self.receive_narrow = context.request.narrow;
        self.frequency = context.transmit.unwrap_or(context.request.frequency);
        self.narrow = context.tx_narrow();
        self.slope = context.slope;
        self.tx_signal = context.tx_signal;
        self.ham = context.ham;
        self.power = context.power;
        self.budget = context.tot;
        self.sounds = SoundSettings { roger: 0, permit: 0, boot: false };
        self.fsk_status = 1;
        self.fsk_message = None;
        self.automatic_ack = Some(packet);
        self.ack_completed = false;
        self.started = now;
        self.deadline = now.wrapping_add(self.budget);
        self.acquisition = crate::app::ptt_check::Check::new(now);
        self.phase = Phase::Acquire;
        self.release.lock();
        self.error = 0;
        self.notice.report(now, 0);
    }

    #[inline(never)]
    pub(crate) fn pump(
        &mut self,
        now: u32,
        context: &mut Context,
        receiver: &mut Receiver,
        sounds: SoundSettings,
    ) {
        self.now = now;

        #[cfg(feature = "fsk")]
        if self.automatic() {
            // This call owns the detached context. Update only automatic
            // admission fields; no second aggregate or external state changes.
            context.allowed = context.fsk_ack_allowed;
            context.fsk_status = 1;
            context.fsk_message = None;
        }

        self.notice.poll(now);

        #[cfg(feature = "cw")]
        if !self.engaged() && self.cw_pitch != context.cw_pitch {
            // Entering or leaving CW changes the meaning of PTT context.
            // A held physical PTT cannot inherit the synthetic session's
            // release qualification when the CW screen closes.
            self.cw_pitch = context.cw_pitch;
            self.release.lock();
            self.previous_ptt = context.ptt;
        }

        let fresh = context.ptt && !self.previous_ptt;
        self.previous_ptt = context.ptt;
        let vox_fresh = !cfg!(feature = "cw") && context.vox_request && !self.previous_vox;
        self.previous_vox = !cfg!(feature = "cw") && context.vox_request;
        let released_before_press = self.release.ready && !self.release.blocked;
        self.release.sample(now, context.ptt || context.raw_ptt);

        if matches!(self.phase, Phase::Alert) {
            if !context.allowed {
                self.local.cancel();
                self.phase = Phase::Idle;
            } else {
                if let Err(error) = self.local.pump(now) { startup::stop(error); }
                if !self.local.active() { self.phase = Phase::Idle; }
            }
            return;
        }

        if self.acquiring() {
            self.pump_acquisition(now, context, receiver);
            return;
        }

        #[cfg(feature = "cw")]
        if matches!(self.phase, Phase::Practice) {
            self.pump_practice(now, context);
            return;
        }

        if self.engaged() {
            if !cfg!(feature = "cw") && self.vox_attempt && (context.ptt || context.raw_ptt) {
                self.stop(0x7503);
                return;
            }
            if self.automatic() && (context.ptt || context.raw_ptt) {
                self.stop(0x7503);
                return;
            }

            let policy = tx_policy::check(
                self.frequency, self.narrow, context.ham, context.power,
            );
            if let Err(error) = policy {
                self.stop(error);
                return;
            }
            if !self.unchanged(context) {
                self.stop(0x7007);
                return;
            }
            if reached(now, self.deadline) {
                self.timeouts = self.timeouts.saturating_add(1);
                self.stop(0x7006);
                return;
            }
            if let Err(error) = Self::battery(self.profile, context.battery) {
                self.stop(error);
                return;
            }
            if !matches!(context.calibration, Ok(profile) if profile == self.profile) {
                self.stop(0x7008);
                return;
            }

            if matches!(self.phase, Phase::Tail) {
                if context.raw_ptt {
                    self.stop(0x7503);
                } else {
                    let result = verify_transmit(self.expected)
                        .and_then(|_| self.check_tuning());
                    if let Err(error) = result {
                        self.stop(error);
                    } else if self.roger.pump(now) {
                        let error = self.roger.error();
                        #[cfg(feature = "fsk")]
                        if self.automatic() {
                            self.ack_completed = error == 0;
                        }
                        self.stop(error);
                    }
                }
                return;
            }

            #[cfg(feature = "fsk")]
            if let Some(packet) = self.automatic_ack {
                if matches!(self.phase, Phase::On) {
                    if self.deadline.wrapping_sub(now) < 1200 {
                        self.stop(0x7006);
                        return;
                    }
                    self.phase = Phase::Tail;
                    self.roger.start_ack(now, packet);
                    return;
                }
            }

            if !cfg!(feature = "cw") && self.vox_attempt && !context.vox_request {
                let remaining = self.deadline.wrapping_sub(now);
                if context.vox_complete && matches!(self.phase, Phase::On)
                    && self.sounds.roger != 0
                    && remaining >= 600 && remaining < 0x8000_0000
                {
                    // Synthetic tail audio must not reach the comparator.
                    // Release its registers and mask before the ordinary
                    // tail owner starts. MIC ADC exclusion, PA profile,
                    // RF checks, and the original TOT remain unchanged.
                    self.vox.stop();
                    write(0x3F, 0);
                    self.phase = Phase::Tail;
                    if let Err(error) = self.configure(0x30, 0xC1FA) {
                        self.stop(error);
                        return;
                    }
                    if let Err(error) = verify_transmit(self.expected) {
                        self.stop(error);
                        return;
                    }
                    self.roger.start(now, self.sounds.roger);
                } else {
                    // Input loss, cancellation, setup expiry, and an
                    // insufficient remaining budget have no Roger tail.
                    self.stop(0);
                }
                return;
            }

            if !context.raw_ptt && !self.automatic() && !self.vox_attempt {
                #[cfg(feature = "cw")]
                if self.cw_active() {
                    let completed = context.cw_complete
                        && matches!(self.phase, Phase::On)
                        && !self.sidetone.down() && self.expected[2] == 0
                        && (self.expected == self.cw_controls(false)
                            || self.breakin.in_gap());

                    // stop verifies PA and links off before returning.
                    // Earlier policy, request, TOT, battery, and calibration
                    // failures never reach this normal completion path.
                    self.stop(0);
                    if completed {
                        self.release.accept_cw_release(now);
                    }
                    return;
                }

                let remaining = self.deadline.wrapping_sub(now);

                #[cfg(feature = "fsk")]
                if matches!(self.phase, Phase::On) && self.fsk_status != 0 {
                    if remaining < 1200 || remaining >= 0x8000_0000 {
                        self.stop(0x7006);
                        return;
                    }

                    // Retain the existing PA profile and attempt deadline.
                    // Disable MIC ADC through the established tail control
                    // before the packet owner enables synthetic modulation.
                    self.phase = Phase::Tail;
                    if let Err(error) = self.configure(0x30, 0xC1FA) {
                        self.stop(error);
                        return;
                    }
                    if let Err(error) = verify_transmit(self.expected) {
                        self.stop(error);
                        return;
                    }

                    if let Some(message) = self.fsk_message {
                        self.roger.start_message(now, message);
                    } else {
                        self.roger.start_fsk(now, self.fsk_status);
                    }
                    return;
                }

                if matches!(self.phase, Phase::On) && self.sounds.roger != 0
                    && remaining >= 600 && remaining < 0x8000_0000
                {
                    self.phase = Phase::Tail;
                    if let Err(error) = self.configure(0x30, 0xC1FA) {
                        self.stop(error);
                        return;
                    }
                    if let Err(error) = verify_transmit(self.expected) {
                        self.stop(error);
                        return;
                    }
                    self.roger.start(now, self.sounds.roger);
                } else {
                    self.stop(0);
                }
                return;
            }

            match self.phase {
                Phase::Permit => {
                    if let Err(error) = self.local.pump(now) {
                        self.stop(error);
                    } else if !self.local.active() {
                        self.phase = Phase::Setup;
                    }
                }
                Phase::Delay => {
                    if reached(now, self.wake_at) {
                        self.phase = Phase::Setup;
                        if self.cw_active() {
                            self.pump_setup(now);
                        }
                    }
                }
                Phase::Setup => self.pump_setup(now),                Phase::On => {
                    #[cfg(feature = "cw")]
                    if self.cw_active() {
                        if self.breakin.active() {
                            self.pump_breakin(
                                now, context.cw_down, context.cw_prepare,
                            );
                            return;
                        }
                        if let Err(error) = self.cw_key(context.cw_down) {
                            self.stop(error);
                            return;
                        }
                    }

                    if !cfg!(feature = "cw") && self.vox_attempt {
                        self.vox.pump(now);
                    }
                    if reached(now, self.next_check) {
                        if let Err(error) = self.check(true) {
                            self.stop(error);
                        } else {
                            self.next_check = now.wrapping_add(100);
                        }
                    }
                }
                #[cfg(feature = "cw")]
                Phase::Practice => {}
                Phase::Idle | Phase::Alert | Phase::Tail | Phase::Acquire => {}
            }
            return;
        }

        self.ham = context.ham;
        self.power = context.power;

        if fresh || vox_fresh {
            self.voice = context.voice;
            self.vox_attempt = !fresh && vox_fresh;
            self.vox_threshold = context.vox_threshold;
            if !cfg!(feature = "cw") && self.vox_attempt {
                if context.ptt || context.raw_ptt || self.automatic()
                    || self.vox_threshold == 0 || self.vox_threshold > 2047
                    || context.request.am
                {
                    self.reject(now, 0x7C01, receiver);
                    return;
                }
                #[cfg(feature = "cw")]
                if context.cw_pitch != 0 {
                    self.reject(now, 0x7C02, receiver);
                    return;
                }
                #[cfg(feature = "fsk")]
                if context.fsk_status != 0 {
                    self.reject(now, 0x7C02, receiver);
                    return;
                }
            }

            #[cfg(feature = "cw")]
            {
                self.cw_pitch = context.cw_pitch;
                self.cw_qsk = context.cw_qsk;
                self.cw_volume = context.cw_volume;
                self.cw_breakin = context.cw_breakin;
                self.cw_tail = context.cw_tail;
            }

            #[cfg(feature = "fsk")]
            {
                self.fsk_status = context.fsk_status;
                self.fsk_message = context.fsk_message;
            }

            self.receive_frequency = context.request.frequency;
            self.receive_signal = context.request.rx_signal;
            self.receive_am = context.request.am;
            self.receive_narrow = context.request.narrow;
            self.slope = context.slope;
            self.frequency = context.transmit.unwrap_or(context.request.frequency);
            self.narrow = context.tx_narrow();

            if !released_before_press || (!self.vox_attempt && !context.raw_ptt) {
                self.reject(now, 0x7009, receiver);
                return;
            }

            #[cfg(feature = "cw")]
            if self.training() {
                if let Err(error) = self.begin_practice(now, context, receiver) {
                    // begin_practice captures cleanup state before touching
                    // local hardware. A pre-admission refusal uses the normal
                    // rejection path without attempting stale TX restoration.
                    if matches!(self.phase, Phase::Practice) {
                        self.stop(error);
                    } else {
                        self.reject(now, error, receiver);
                    }
                }
                return;
            }

            let acquire = context.acquisition_required();
            let admission = if acquire {
                Self::evaluate(context, false)
            } else {
                Self::admit(context)
            };

            match admission {
                Ok(profile) => {
                    self.profile = profile;
                    self.tx_signal = context.tx_signal;
                    self.sounds = if self.cw_active() {
                        SoundSettings { roger: 0, permit: 0, boot: false }
                    } else if !cfg!(feature = "cw") && self.vox_attempt {
                        // Automatic voice starts without a local cue.
                        // Normal hold completion retains selected Roger.
                        SoundSettings { permit: 0, ..sounds }
                    } else if self.receive_am {
                        SoundSettings { roger: 0, ..sounds }
                    } else {
                        sounds
                    };
                    self.budget = context.tot;

                    if acquire {
                        // No TX hardware is admitted here. The receiver
                        // restores the requested profile, then supplies fresh
                        // carrier observations under the fixed acquisition
                        // deadline. Keyer input remains pending until On.
                        self.started = now;
                        self.deadline = now.wrapping_add(self.budget);
                        self.acquisition = crate::app::ptt_check::Check::new(now);
                        self.phase = Phase::Acquire;
                        self.release.lock();
                        self.error = 0; self.notice.report(self.now, 0);
                    } else {
                        self.begin(now, receiver);
                    }
                }
                Err(error) => self.reject(now, error, receiver),
            }
            return;
        }

        #[cfg(feature = "fsk")]
        self.try_ack(now, context);
    }

    pub(crate) fn publish(&self, now: u32) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let view = self.view();
        let state = TransmitState {
            signature: 0, tick: now, phase: self.phase as u32,
            ham: self.ham as u32, frequency: self.frequency,
            bias: self.profile.bias as u32, pa: self.profile.pa as u32,
            elapsed: view.elapsed, error: self.error,
            starts: self.starts, timeouts: self.timeouts,
            reg30: self.expected[0] as u32,
            reg33: self.expected[1] as u32,
            reg36: self.expected[2] as u32,
            checks: self.checks,
            audio_readback: ((self.audio_expected as u32) << 16)
                | self.audio_actual as u32,
        };
        crate::observation::publish!(TRAKT_TRANSMITTER, state, 0x5454_4354);
    }
}

const _: () = {
    let profile = Profile {
        bias: 15,
        pa: 0x0F88,
        gpio: 0x9025,
        crystal: 22654,
        battery_min: 1993,
        battery_max: 2860,
    };

    let mut context = Context {
        #[cfg(feature = "cw")]
        cw_pitch: 0,
        #[cfg(feature = "cw")]
        cw_down: false,
        #[cfg(feature = "cw")]
        cw_prepare: false,
        #[cfg(feature = "cw")]
        cw_complete: false,
        #[cfg(feature = "cw")]
        cw_qsk: true,
        #[cfg(feature = "cw")]
        cw_volume: 28,
        #[cfg(feature = "cw")]
        cw_breakin: 0,
        #[cfg(feature = "cw")]
        cw_tail: 0,
        #[cfg(feature = "fsk")]
        fsk_status: 0,
        #[cfg(feature = "fsk")]
        fsk_message: None,
        #[cfg(feature = "fsk")]
        fsk_ack: None,
        #[cfg(feature = "fsk")]
        fsk_ack_allowed: false,
        vox_request: false,
        vox_complete: false,
        vox_threshold: 128,
        voice: crate::voice::Settings::new(),
        dual_watch: false,
        slope: crate::slope::Settings::new(),
        transmit: Ok(145_500_000),
        tx_signal: 0,
        allowed: true,
        request: Request {
            enabled: true,
            frequency: 145_500_000,
            am: false, narrow: false,
            monitor: false,
            squelch: 3,
            rx_signal: 0,
        },
        receiver: RxView {
            phase: RxPhase::Receive,
            frequency: 145_500_000,
            am: false, narrow: false,
            rssi: 0,
            noise: 127,
            audio: false,
            monitor: false,
            rx_signal: 0,
            tone_debug: [0; 4],
            error: 0,
            signal: false,
            glitch: 0,
            sample_number: 1,
        },
        calibration: Ok(profile),
        battery: Reading {
            average: Some(2170),
            raw: Some(2170),
            error: 0,
        },
        ptt: true,
        raw_ptt: true,
        ham: true,
        power: 0,
        tot: 30000,
    };

    assert!(Transmitter::admit(&context).is_ok());
    assert!(!context.acquisition_required());

    #[cfg(feature = "fsk")]
    {
        context.fsk_ack = Some(crate::link::Packet::Ack {
            sender: 1, destination: 2, sequence: 3,
        });
        context.fsk_ack_allowed = true;
        context.allowed = false;
        assert!(matches!(Transmitter::admit(&context), Err(0x7001)));
        context.allowed = true;
        context.fsk_ack = None;
        context.fsk_ack_allowed = false;
    }

    #[cfg(feature = "cw")]
    {
        context.cw_pitch = 700;
        context.request.narrow = true;
        context.receiver.narrow = true;
        assert!(!context.cw_complete);
        assert!(Transmitter::admit(&context).is_ok());
        assert!(!context.acquisition_required());

        context.receiver.phase = RxPhase::Settle;
        assert!(context.acquisition_required());
        assert!(Transmitter::evaluate(&context, false).is_ok());
        assert!(matches!(Transmitter::admit(&context), Err(0x7003)));

        context.receiver.phase = RxPhase::Receive;
        context.receiver.signal = true;
        assert!(!context.acquisition_required());
        assert!(matches!(Transmitter::admit(&context), Err(0x7005)));

        context.receiver.signal = false;
        context.receiver.frequency = 433_500_000;
        assert!(context.acquisition_required());
        context.receiver.frequency = 145_500_000;
        assert!(!context.acquisition_required());

        context.cw_pitch = 0;
        context.request.narrow = false;
        context.receiver.narrow = false;
    }

    context.dual_watch = true;
    assert!(context.acquisition_required());
    context.dual_watch = false;

    context.request.am = true;
    assert!(matches!(Transmitter::admit(&context), Err(0x700F)));
    assert!(matches!(Transmitter::evaluate(&context, false), Err(0x700F)));
    context.ham = false;
    context.transmit = Ok(433_700_000);
    assert!(matches!(Transmitter::admit(&context), Err(0x700F)));
    context.ham = true;
    context.transmit = Ok(145_500_000);
    context.request.am = false;
    context.receiver.am = true;
    assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
    context.receiver.am = false;

    context.request.am = true;
    context.receiver.am = true;
    context.slope.enabled = true;
    context.transmit = Ok(145_495_000);

    if cfg!(feature = "fam") {
        assert!(Transmitter::admit(&context).is_ok());

        // TX width is independent of AM receive width.
        context.slope.narrow = true;
        assert!(Transmitter::admit(&context).is_ok());
        context.slope.narrow = false;

        context.transmit = Ok(145_500_000);
        assert!(matches!(Transmitter::admit(&context), Err(0x7013)));
        context.transmit = Ok(145_495_000);
        context.tx_signal = 885;
        assert!(matches!(Transmitter::admit(&context), Err(0x7013)));
        context.tx_signal = 0;

        context.receiver.signal = true;
        assert!(matches!(Transmitter::admit(&context), Err(0x7005)));
        context.receiver.signal = false;

        // Nominal is admitted but the shifted wide signal crosses the
        // configured HAM margin. The actual frequency must also pass.
        context.request.frequency = 144_012_500;
        context.receiver.frequency = 144_012_500;
        context.transmit = Ok(144_007_500);
        assert!(matches!(Transmitter::admit(&context), Err(0x7601)));

        context.request.frequency = 145_500_000;
        context.receiver.frequency = 145_500_000;
        context.transmit = Ok(145_495_000);

        let mut captured = Transmitter::new();
        captured.receive_frequency = 145_500_000;
        captured.receive_am = true;
        captured.receive_narrow = false;
        captured.slope = context.slope;
        captured.frequency = 145_495_000;
        assert!(captured.unchanged(&context));

        context.slope.delta = 40;
        assert!(!captured.unchanged(&context));
        context.slope.delta = 50;
        context.request.am = false;
        assert!(!captured.unchanged(&context));
        context.request.am = true;
        context.request.narrow = true;
        assert!(!captured.unchanged(&context));
        context.request.narrow = false;
    } else {
        assert!(matches!(Transmitter::admit(&context), Err(0x700F)));
        assert!(matches!(Transmitter::evaluate(&context, false), Err(0x700F)));
    }

    context.request.am = false;
    context.receiver.am = false;
    context.slope = crate::slope::Settings::new();
    context.transmit = Ok(145_500_000);

    // The other VFO is not a valid final RX admission snapshot.
    // Early acquisition checks policy without using that stale view.
    context.receiver.frequency = 433_700_000;
    context.receiver.signal = true;
    assert!(Transmitter::evaluate(&context, false).is_ok());
    assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
    context.allowed = false;
    assert!(matches!(Transmitter::evaluate(&context, false), Err(0x7001)));
    context.allowed = true;
    context.receiver.frequency = 145_500_000;
    assert!(matches!(Transmitter::admit(&context), Err(0x7005)));
    context.receiver.signal = false;
    assert!(Transmitter::admit(&context).is_ok());

    context.tx_signal = 1000;
    assert!(Transmitter::admit(&context).is_ok());
    context.tx_signal = 1001;
    assert!(matches!(Transmitter::admit(&context), Err(0x700D)));
    context.tx_signal = 0x8013;
    assert!(Transmitter::admit(&context).is_ok());
    context.tx_signal = 0xC013;
    assert!(Transmitter::admit(&context).is_ok());
    context.tx_signal = 0x8213;
    assert!(matches!(Transmitter::admit(&context), Err(0x700D)));
    context.tx_signal = 0;

    // Selection checks use an already supplied valid calibration result.
    // PA-code derivation has separate factory-table tests.
    let mut power = 0;
    while power < crate::app::tx_options::POWER_COUNT {
        context.power = power;
        assert!(Transmitter::admit(&context).is_ok());
        power += 1;
    }
    context.power = crate::app::tx_options::POWER_COUNT;
    assert!(matches!(Transmitter::admit(&context), Err(0x7604)));
    context.power = 0;

    let mut selection = 0u8;
    while (selection as u32) < crate::app::tx_options::timeout_count() {
        context.tot = match crate::app::tx_options::timeout_ticks(selection) {
            Some(ticks) => ticks,
            None => panic!("timeout selection rejected"),
        };
        assert!(Transmitter::admit(&context).is_ok());
        selection += 1;
    }
    context.tot = 0;
    assert!(matches!(Transmitter::admit(&context), Err(0x700A)));
    context.tot = 30001;
    assert!(matches!(Transmitter::admit(&context), Err(0x700A)));
    context.tot = 30000;

    context.allowed = false;
    assert!(matches!(Transmitter::admit(&context), Err(0x7001)));
    context.allowed = true;

    context.request.enabled = false;
    assert!(matches!(Transmitter::admit(&context), Err(0x7001)));
    context.request.enabled = true;

    context.request.monitor = true;
    assert!(matches!(Transmitter::admit(&context), Err(0x7001)));
    context.request.monitor = false;

    context.request.squelch = 0;
    assert!(matches!(Transmitter::admit(&context), Err(0x7001)));
    context.request.squelch = 3;

    let unsettled = [
        RxPhase::Off, RxPhase::Wait, RxPhase::Program,
        RxPhase::Settle, RxPhase::Failed,
    ];
    let mut index = 0;
    while index < unsettled.len() {
        context.receiver.phase = unsettled[index];
        assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
        index += 1;
    }
    context.receiver.phase = RxPhase::Receive;

    context.receiver.frequency = 145_525_000;
    assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
    context.receiver.frequency = context.request.frequency;

    context.receiver.narrow = true;
    assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
    context.receiver.narrow = false;

    context.request.rx_signal = 885;
    assert!(matches!(Transmitter::admit(&context), Err(0x7003)));
    context.receiver.rx_signal = 885;
    context.receiver.signal = true;
    context.receiver.audio = false;
    assert!(matches!(Transmitter::admit(&context), Err(0x7005)));
    context.receiver.signal = false;
    context.request.rx_signal = 0;
    context.receiver.rx_signal = 0;

    context.calibration = Err(0x7304);
    assert!(matches!(Transmitter::admit(&context), Err(0x7304)));
    context.calibration = Ok(profile);

    context.battery.raw = None;
    assert!(matches!(Transmitter::admit(&context), Err(0x7004)));
    context.battery.raw = Some(2170);
    context.battery.error = 1;
    assert!(matches!(Transmitter::admit(&context), Err(0x7004)));
    context.battery.error = 0;

    context.battery.raw = Some(1992);
    assert!(matches!(Transmitter::admit(&context), Err(0x7004)));
    context.battery.raw = Some(1993);
    assert!(Transmitter::admit(&context).is_ok());
    context.battery.raw = Some(2860);
    assert!(Transmitter::admit(&context).is_ok());
    context.battery.raw = Some(2861);
    assert!(matches!(Transmitter::admit(&context), Err(0x7004)));
    context.battery.raw = Some(2170);

    context.request.frequency = 150_000_000;
    context.receiver.frequency = 150_000_000;
    context.transmit = Ok(150_000_000);
    assert!(matches!(Transmitter::admit(&context), Err(0x7601)));
    context.ham = false;
    assert!(Transmitter::admit(&context).is_ok());

    // RX outside the HAM allocation does not invalidate an admitted TX
    // destination. Conversely, an admitted RX frequency cannot authorize
    // a forbidden TX destination.
    context.ham = true;
    context.transmit = Ok(145_500_000);
    assert!(Transmitter::admit(&context).is_ok());
    context.request.frequency = 145_500_000;
    context.receiver.frequency = 145_500_000;
    context.transmit = Ok(150_000_000);
    assert!(matches!(Transmitter::admit(&context), Err(0x7601)));

    context.transmit = Err(0x700B);
    assert!(matches!(Transmitter::admit(&context), Err(0x700B)));
    context.transmit = Err(0x700C);
    assert!(matches!(Transmitter::admit(&context), Err(0x700C)));

    context.transmit = Ok(433_700_000);
    context.calibration = Ok(Profile {
        pa: 0x0FA2, gpio: 0x9029, ..profile
    });
    assert!(Transmitter::admit(&context).is_ok());

    let mut frozen = Transmitter::new();
    frozen.receive_frequency = 145_500_000;
    frozen.frequency = 433_700_000;
    frozen.narrow = false;
    frozen.ham = true;
    frozen.power = 0;
    frozen.budget = 30000;
    assert!(frozen.unchanged(&context));
    context.tx_signal = 1000;
    assert!(!frozen.unchanged(&context));
    context.tx_signal = 0;

    context.request.frequency = 145_525_000;
    assert!(!frozen.unchanged(&context));
    context.request.frequency = 145_500_000;
    context.transmit = Ok(433_725_000);
    assert!(!frozen.unchanged(&context));
    context.transmit = Ok(433_700_000);
    context.tot = 60000;
    assert!(!frozen.unchanged(&context));
    context.tot = 30000;
    context.allowed = false;
    assert!(!frozen.unchanged(&context));
};

const _: () = {
    assert!(core::mem::size_of::<TransmitState>() == 64);
    assert!(MIC_CONFIG == 0xE940);
    assert!(MIC_CONFIG & 0x3F == 0);

    let mut microphone = Transmitter::new();
    microphone.original_audio[0] = 0xE920;
    assert!(microphone.microphone_config() == 0xE940);
    microphone.vox_attempt = true;
    assert!(microphone.microphone_config() == if cfg!(feature = "cw") { 0xE940 } else { 0xE920 });
    microphone.step = 9;
    assert!(matches!(microphone.sequence(), Step::Microphone));
    microphone.vox_attempt = false;
    assert!(microphone.microphone_config() == 0xE940);
    assert!(matches!(microphone.sequence(), Step::Microphone));

    let mut release = Release::new();
    assert!(!release.ready && release.blocked);
    release.sample(0, true);
    release.sample(500, true);
    assert!(!release.ready);
    release.sample(501, false);
    release.sample(600, false);
    assert!(!release.ready);
    release.sample(601, false);
    assert!(release.ready && !release.blocked);

    // Physical press arrives before the foreground receives PTT Down.
    release.sample(602, true);
    assert!(release.ready && !release.blocked);
    release.sample(631, true);
    assert!(release.ready && !release.blocked);
    release.sample(632, true);
    assert!(release.ready && !release.blocked);

    // Accepting or rejecting the attempt consumes release qualification.
    release.lock();
    release.sample(650, true);
    assert!(!release.ready && release.blocked);
    assert!(!release.ready && release.blocked);
    release.sample(700, false);
    release.sample(800, false);
    assert!(release.ready && !release.blocked);

    let mut wrapped = Release::new();
    wrapped.sample(u32::MAX - 50, false);
    wrapped.sample(48, false);
    assert!(!wrapped.ready);
    wrapped.sample(49, false);
    assert!(wrapped.ready);

    let mut setup = Transmitter::new();
    setup.frequency = 145_500_000;
    setup.profile = Profile {
        bias: 15, pa: 0x0F88, gpio: 0x9025,
        crystal: 22_654, battery_min: 1993, battery_max: 2860,
    };

    let mut index = 0;
    while index < 24 {
        setup.step = index;
        if let Step::Write(register, value) = setup.sequence() {
            match register {
                0x30 => assert!(matches!(value, 0 | 0xC1FE)),
                0x33 => assert!(value == 0x9000),
                0x36 => assert!(value == 0),
                _ => {}
            }
        }
        index += 1;
    }
    setup.step = 23;
    assert!(matches!(setup.sequence(), Step::Controls));
    setup.step = 24;
    assert!(matches!(setup.sequence(), Step::Write(0x33, 0x9025)));
    setup.step = 26;
    assert!(matches!(setup.sequence(), Step::Write(0x36, 0x0F88)));

};
const _: () = {
    let mut setup = Transmitter::new();
    setup.receive_am = true;
    setup.step = 18;
    assert!(matches!(setup.sequence(), Step::Fm));
    setup.receive_am = false;
    assert!(matches!(setup.sequence(), Step::Write(0x40, 0x3516)));

    // Both paths retain the original PA admission positions.
    setup.receive_am = true;
    setup.step = 23;
    assert!(matches!(setup.sequence(), Step::Controls));
    setup.step = 19;
    assert!(matches!(setup.sequence(), Step::Write(0x30, 0xC1FE)));
};

#[cfg(feature = "cw")]
const _: () = {
    let mut setup = Transmitter::new();
    setup.cw_pitch = 700;
    setup.profile = Profile {
        bias: 15, pa: 0x0F88, gpio: 0x9025,
        crystal: 22_654, battery_min: 1993, battery_max: 2860,
    };

    let mark = setup.cw_controls(true);
    let space = setup.cw_controls(false);
    assert!(mark[0] == 0xC1FA && space[0] == 0xC1F2);
    assert!(mark[0] & 4 == 0 && space[0] & 4 == 0);
    assert!(mark[0] ^ space[0] == 8);
    assert!(mark[1] == 0x9025 && mark[2] == 0x0F88);
    assert!(space[1] == 0x9000 && space[2] == 0);
    assert!(mark[3] == space[3] && mark[4] == space[4]);

    setup.step = 19;
    assert!(matches!(setup.sequence(), Step::Write(0x30, 0xC1F2)));
    setup.step = 23;
    assert!(matches!(setup.sequence(), Step::Controls));
    setup.step = 24;
    assert!(matches!(setup.sequence(), Step::Write(0x33, 0x9000)));
    setup.step = 26;
    assert!(matches!(setup.sequence(), Step::Write(0x36, 0)));

    setup.profile.gpio = 0x9029;
    setup.profile.pa = 0x0FA2;
    let uhf = setup.cw_controls(true);
    assert!(uhf[1] == 0x9029 && uhf[2] == 0x0FA2);
    assert!(setup.cw_controls(false)[1] == 0x9000);

    // A partially elapsed audio interval waits for its original deadline,
    // not for another seventy ticks from the end of RF preparation.
    setup.phase = Phase::Setup;
    setup.step = 31;
    setup.cw_audio_at = 170;
    assert!(matches!(setup.sequence(), Step::CwAudio));
    setup.finish_cw_audio(150);
    assert!(setup.step == 32);
    assert!(matches!(setup.phase, Phase::Delay));
    assert!(setup.wake_at == 170);

    // Audio which settled during RF setup adds no further Delay phase.
    setup.phase = Phase::Setup;
    setup.step = 31;
    setup.finish_cw_audio(170);
    assert!(matches!(setup.phase, Phase::Setup));
    assert!(matches!(setup.sequence(), Step::Finish));

    setup.phase = Phase::Setup;
    setup.step = 31;
    setup.finish_cw_audio(200);
    assert!(matches!(setup.phase, Phase::Setup));
    assert!(matches!(setup.sequence(), Step::Finish));

    // Absolute deadline comparisons retain the ordinary wrapping contract.
    setup.phase = Phase::Setup;
    setup.step = 31;
    setup.cw_audio_at = 19;
    setup.finish_cw_audio(u32::MAX - 10);
    assert!(matches!(setup.phase, Phase::Delay));
    assert!(setup.wake_at == 19);

    setup.phase = Phase::Setup;
    setup.step = 31;
    setup.finish_cw_audio(19);
    assert!(matches!(setup.phase, Phase::Setup));
    assert!(matches!(setup.sequence(), Step::Finish));

    // RF waits and admission checkpoints retain their original positions.
    setup.step = 20;
    assert!(matches!(setup.sequence(), Step::Wait(10)));
    setup.step = 22;
    assert!(matches!(setup.sequence(), Step::Wait(2)));
    setup.step = 23;
    assert!(matches!(setup.sequence(), Step::Controls));
    setup.step = 25;
    assert!(matches!(setup.sequence(), Step::Wait(5)));
    setup.step = 27;
    assert!(matches!(setup.sequence(), Step::Wait(10)));
    setup.step = 28;
    assert!(matches!(setup.sequence(), Step::Check));
    setup.step = 30;
    assert!(matches!(setup.sequence(), Step::Wait(2)));

    let mut release = Release::new();
    assert!(release.blocked && !release.ready);
    release.accept_cw_release(500);
    assert!(release.ready && !release.blocked);
    assert!(release.observing && release.deadline == 500);

    // A new assertion may consume the already completed release.
    release.sample(501, true);
    assert!(release.ready && !release.blocked);

    // Beginning or rejecting another attempt still consumes qualification.
    release.lock();
    release.sample(502, true);
    release.sample(1000, true);
    assert!(!release.ready && release.blocked);
    release.sample(1001, false);
    release.sample(1100, false);
    assert!(!release.ready && release.blocked);
    release.sample(1101, false);
    assert!(release.ready && !release.blocked);

    // Accepted completion does not need another delay across tick wrap.
    release.lock();
    release.accept_cw_release(u32::MAX);
    release.sample(0, true);
    assert!(release.ready && !release.blocked);
};

