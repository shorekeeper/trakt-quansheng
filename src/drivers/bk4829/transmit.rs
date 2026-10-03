//! Foreground TX ownership with normal PTT and independent admission.
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
}

enum Step {
    Write(u8, u16),
    Subtone,
    Fm,
    Wait(u32),
    Controls,
    Check,
    Finish,
}

pub(crate) struct Context {
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

    /// This engaged phase owns no TX hardware and requires receiver pumping.
    pub(crate) const fn acquiring(&self) -> bool {
        matches!(self.phase, Phase::Acquire)
    }

    pub(crate) fn voice_active(&self) -> bool {
        matches!(self.phase, Phase::On)
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
                Phase::Acquire => b"CHECK SELECTED RX",
                Phase::Alert => b"TX ERROR",
                Phase::Permit => b"LOCAL PERMIT",
                Phase::Setup | Phase::Delay => b"TX SETUP",
                Phase::On if self.receive_am => b"SC-FM TRANSMIT",
                Phase::On => b"TRANSMITTING",
                Phase::Tail => b"ROGER TAIL",
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
            if !matches!(context.receiver.phase, RxPhase::Receive)
                || context.receiver.frequency != context.request.frequency
                || context.receiver.narrow != context.request.narrow
                || context.receiver.am != context.request.am
                || context.receiver.rx_signal != context.request.rx_signal
            {
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
            && matches!(context.transmit, Ok(frequency) if frequency == self.frequency)
    }

    fn observe_audio(&mut self, talking: bool) {
        self.audio_expected = if talking { 0x3B18 } else { 0xBB18 };
        self.audio_actual = read(0x50);
    }

    fn check_tuning(&self) -> Result<(), u32> {
        let frequency = read(0x38) as u32 | ((read(0x39) as u32) << 16);
        if frequency != self.frequency / 10 { return Err(0x7238); }
        if read(0x43) != if self.narrow { 0x4048 } else { 0x3028 } {
            return Err(0x7243);
        }
        if read(0x3B) != self.profile.crystal { return Err(0x723B); }
        if read(0x40) != 0x3516 { return Err(0x7240); }
        if read(0x7D) != MIC_CONFIG { return Err(0x727D); }
        if read(0x31) & 15 != 0
            || (!matches!(self.phase, Phase::Tail) && read(0x70) != 0)
        {
            return Err(0x7231);
        }
        if self.receive_am {
            super::demod::check(false)?;
        }
        if read(0x51) != super::subtone::tx_control(self.tx_signal) {
            return Err(0x7251);
        }
        Ok(())
    }

    fn check_setup(&mut self) -> Result<(), u32> {
        if self.expected != [0xC1FE, 0x9000, 0, 0x9D1F, 0x6042] {
            return Err(0x7201);
        }
        let _ = verify_transmit(self.expected)?;
        self.check_tuning()?;
        self.observe_audio(false);
        self.checks = self.checks.saturating_add(1);
        Ok(())
    }

    fn check(&mut self, talking: bool) -> Result<(), u32> {
        let actual = verify_transmit(self.expected)?;
        if actual[0] & 0x7FFF != self.expected[0] & 0x7FFF
            || actual[1] != self.profile.gpio
            || actual[2] != self.profile.pa
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
        if !context.raw_ptt {
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

        let settled = matches!(context.receiver.phase, RxPhase::Receive)
            && context.receiver.frequency == context.request.frequency
            && context.receiver.narrow == context.request.narrow
            && context.receiver.am == context.request.am
            && context.receiver.rx_signal == context.request.rx_signal;

        match self.acquisition.poll(
            now, settled, context.receiver.signal, context.receiver.sample_number,
        ) {
            Ok(false) => {}
            Err(error) => self.reject(now, error, receiver),
            Ok(true) => match Self::admit(context) {
                Ok(_) => self.begin(now, receiver),
                Err(error) => self.reject(now, error, receiver),
            },
        }
    }

    fn reject(&mut self, now: u32, error: u32, receiver: &mut Receiver) {
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
            2 => Step::Write(0x30, 0),
            3 => Step::Write(0x47, 0x6042),
            4 => Step::Write(0x50, 0xBB18),
            5 => Step::Write(0x38, frequency as u16),
            6 => Step::Write(0x39, (frequency >> 16) as u16),
            7 => Step::Write(0x43, if self.narrow { 0x4048 } else { 0x3028 }),
            8 => Step::Write(0x3B, self.profile.crystal),
            9 => Step::Write(0x7D, MIC_CONFIG),
            10 => Step::Write(0x7E, (self.original_audio[1] & !(7 << 3)) | (5 << 3)),
            11 => Step::Write(0x31, self.original_audio[2] & !15),
            12 => Step::Write(0x2B, 0),
            13 => Step::Write(0x24, 0),
            14 => Step::Write(0x70, 0),
            15 => Step::Subtone,
            16 => Step::Write(0x52, 0x028F),
            17 => Step::Write(0x37, 0x9D1F),
            18 if self.receive_am => Step::Fm,
            18 => Step::Write(0x40, 0x3516),
            19 => Step::Write(0x30, 0xC1FE),
            20 => Step::Wait(10),
            21 => Step::Write(0x50, 0xBB18),
            22 => Step::Wait(2),
            23 => Step::Controls,
            24 => Step::Write(0x33, self.profile.gpio),
            25 => Step::Wait(5),
            26 => Step::Write(0x36, self.profile.pa),
            27 => Step::Wait(10),
            28 => Step::Check,
            29 => Step::Write(0x50, 0x3B18),
            30 => Step::Wait(2),
            _ => Step::Finish,
        }
    }

    fn configure(&mut self, register: u8, value: u16) -> Result<(), u32> {
        let permitted = match register {
            0x30 => matches!(value, 0 | 0xC1FE)
                || (value == 0xC1FA && matches!(self.phase, Phase::Tail)),
            0x33 => value == 0x9000 || value == self.profile.gpio,
            0x36 => value == 0 || value == self.profile.pa,
            0x37 => value == 0x9D1F,
            0x47 => value == 0x6042,
            _ => true,
        };
        if !permitted { return Err(0x7400 + register as u32); }

        if register == 0x33 && value != 0x9000 {
            self.check_setup()?;
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

    #[inline(never)]
    pub(crate) fn pump(
        &mut self,
        now: u32,
        context: Context,
        receiver: &mut Receiver,
        sounds: SoundSettings,
    ) {
        self.now = now;

        self.notice.poll(now);
        let fresh = context.ptt && !self.previous_ptt;
        self.previous_ptt = context.ptt;
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
            self.pump_acquisition(now, &context, receiver);
            return;
        }

        if self.engaged() {
            let policy = tx_policy::check(
                self.frequency, self.narrow, context.ham, context.power,
            );
            if let Err(error) = policy {
                self.stop(error);
                return;
            }
            if !self.unchanged(&context) {
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
                        self.stop(0);
                    }
                }
                return;
            }

            if !context.raw_ptt {
                let remaining = self.deadline.wrapping_sub(now);
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
                    if reached(now, self.wake_at) { self.phase = Phase::Setup; }
                }
                Phase::Setup => match self.sequence() {
                    Step::Fm => {
                        // The existing parked setup owns the RF chip.
                        // This restores the AM/FM coefficient pair before PA.
                        super::demod::configure(false);
                        match self.configure(0x40, 0x3516) {
                            Ok(()) => self.step += 1,
                            Err(error) => self.stop(error),
                        }
                    }
                    Step::Subtone => match super::subtone::configure_tx(self.tx_signal) {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    },
                    Step::Write(register, value) => match self.configure(register, value) {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    },
                    Step::Wait(ticks) => {
                        self.step += 1;
                        self.wake_at = now.wrapping_add(ticks);
                        self.phase = Phase::Delay;
                    }
                    Step::Controls => match self.check_setup() {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    },
                    Step::Check => match self.check(false) {
                        Ok(()) => self.step += 1,
                        Err(error) => self.stop(error),
                    },
                    Step::Finish => match self.check(true) {
                        Ok(()) => {
                            self.phase = Phase::On;
                            self.next_check = now.wrapping_add(100);
                        }
                        Err(error) => self.stop(error),
                    },
                },
                Phase::On => {
                    if reached(now, self.next_check) {
                        if let Err(error) = self.check(true) {
                            self.stop(error);
                        } else {
                            self.next_check = now.wrapping_add(100);
                        }
                    }
                }
                Phase::Idle | Phase::Alert | Phase::Tail | Phase::Acquire => {}
            }
            return;
        }

        self.ham = context.ham;
        self.power = context.power;

        if fresh {
            self.receive_frequency = context.request.frequency;
            self.receive_signal = context.request.rx_signal;
            self.receive_am = context.request.am;
            self.receive_narrow = context.request.narrow;
            self.slope = context.slope;
            self.frequency = context.transmit.unwrap_or(context.request.frequency);
            self.narrow = context.tx_narrow();

            if !released_before_press || !context.raw_ptt {
                self.reject(now, 0x7009, receiver);
                return;
            }

            let admission = if context.dual_watch {
                Self::evaluate(&context, false)
            } else {
                Self::admit(&context)
            };

            match admission {
                Ok(profile) => {
                    self.profile = profile;
                    self.tx_signal = context.tx_signal;
                    self.sounds = if self.receive_am {
                        SoundSettings { roger: 0, ..sounds }
                    } else {
                        sounds
                    };
                    self.budget = context.tot;

                    if context.dual_watch {
                        // No RF writes or timer refresh occur here.
                        // The following receiver pump remains muted but
                        // continues carrier SQL on the selected VFO.
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
        }
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
