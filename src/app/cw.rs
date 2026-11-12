//! One Home CW keying session and independent RX/TX decoder histories.
//!
//! Settings are edited by the common native menu. Panel applies confirmed
//! checked words and serializes choices, never contact qualification,
//! pending elements, decoder text, or live deadlines. The separate input
//! observation page cannot request transmission or edit choices.
//!
//! Local contacts are PTT/Side1; USB contacts are tip/ring. USB Test supplies
//! observations but cannot create a TX session. Panel independently denies
//! its TX admission. Unavailable USB never falls back to local contacts.
//!
//! Entry, input loss, settings changes, and reported errors discard keyer
//! memory and require both selected contacts released for 100 observed ticks.
//! Each contact retains its five-tick assertion filter and immediate release.
//! A foreground service gap above twenty ticks cancels the session.
//!
//! An electronic keyer retains initial input during setup and waits for the
//! transmitter's ready phase before starting an element. One initial straight
//! contact released during setup is retained with its observed duration,
//! limited to 5..2000 ticks, and played once after readiness. Errors, input
//! loss, and cancellation discard it. Later straight keying remains live.
//! Session request spans
//! elements and the selected TX hang; it never renews the transmitter's TOT.
//!
//! Normal hang completion may retain an already completed 100-tick physical
//! release observation. Its one-pass completion flag is accepted by the
//! transmitter only after normal verified RF-off cleanup. Errors and timeout
//! cannot use this path to inherit release qualification.
//!
//! RX text consumes the receiver's qualified fast RSSI envelope, independently
//! of carrier SQL and speaker gating. TX text follows the register-controlled
//! mark state after RF service, not an independent RF measurement. Each
//! decoder uses the selected WPM and retains its own bounded history.
//!
//! Home borrows completed text and provisional dot/dash/character projections.
//! Provisional characters never enter history before a complete character
//! gap. Retuning, unavailable RX, and TX ownership discard partial RX timing.
//! Ending TX flushes only complete observed marks and separates sessions.
//!
//! START retains the last successful interval from accepted session request
//! to the first observation of transmitter readiness, before keyer timing
//! begins. It includes any queued RX acquisition and TX/audio setup, but
//! excludes contact filtering before that request. Cancellation preserves
//! the previous completed observation. It is not measured RF onset.
//!
//! Input diagnostics report raw contacts, RSSI thresholds, completed qualified
//! mark/space durations, and maximum sample interval. They allocate neither
//! another decoder nor a framebuffer. Clearing text changes no saved setting.
use crate::cw::{Contact, Keyer, Settings};
use crate::drivers::bk4829::transmit::View as TxView;
#[cfg(feature = "cw-debug")]
use crate::drivers::bk4829::receive::View as RxView;
#[cfg(feature = "cw-debug")]
use crate::ui::{font::Size, widgets, Frame};

/// completed is a one-pass observation after normal hang expiry with both
/// physical contacts released for at least 100 ticks. It is false during
/// marks, setup, ordinary idle, cancellation, and input-only testing.
/// The transmitter may accept it only after verified normal RF shutdown.
#[derive(Clone, Copy)]
pub(super) struct Demand {
    pub(super) pitch: u16,
    pub(super) gated: bool,
    pub(super) session: bool,
    pub(super) down: bool,
    /// Prepare TX without starting a mark or another session.
    pub(super) prepare: bool,
    pub(super) completed: bool,
}

impl Demand {
    pub(super) const fn off() -> Self {
        Self {
            pitch: 0, gated: false, session: false, down: false,
            prepare: false, completed: false,
        }
    }
}

pub(super) struct Lab {
    settings: Settings,

    ptt: Contact,
    side: Contact,
    keyer: Keyer,
    lead: crate::cw_start::Lead,
    rx_decode: crate::cw_decode::Decoder,
    tx_decode: crate::cw_decode::Decoder,
    #[cfg(feature = "cw-debug")]
    rx_probe: Option<crate::cw_decode::Probe>,
    raw_contacts: Option<(bool, bool)>,
    qualified: bool,
    releasing: bool,
    release_at: u32,
    requested: bool,
    pending: u8,
    ready: bool,
    last_activity: u32,
    #[cfg(feature = "cw-debug")]
    request_at: u32,
    #[cfg(feature = "cw-debug")]
    setup_ticks: Option<u32>,
    last_poll: u32,
    polled: bool,
    saw_engaged: bool,
}

impl Lab {
    pub(super) const fn new() -> Self {
        Self {
            settings: Settings::new(),

            ptt: Contact::new(),
            side: Contact::new(),
            keyer: Keyer::new(),
            lead: crate::cw_start::Lead::new(),
            rx_decode: crate::cw_decode::Decoder::new(),
            tx_decode: crate::cw_decode::Decoder::new(),
            #[cfg(feature = "cw-debug")]
            rx_probe: None,
            raw_contacts: None,
            qualified: false,
            releasing: false,
            release_at: 0,
            requested: false,
            pending: 0,
            ready: false,
            last_activity: 0,
            #[cfg(feature = "cw-debug")]
            request_at: 0,
            #[cfg(feature = "cw-debug")]
            setup_ticks: None,
            last_poll: 0,
            polled: false,
            saw_engaged: false,
        }
    }



    /// Preserve choices and completed text while discarding every live
    /// transmission request. No history-sized temporary is constructed.
    // UI transitions and input/RF failures share complete session cancellation.
    #[inline(never)]
    pub(super) fn cancel(&mut self) {
        self.ptt = Contact::new();
        self.side = Contact::new();
        self.keyer = Keyer::new();
        self.lead = crate::cw_start::Lead::new();
        self.qualified = false;
        self.releasing = false;
        self.release_at = 0;
        self.requested = false;
        self.pending = 0;
        self.ready = false;
        self.last_activity = 0;
        self.last_poll = 0;
        self.polled = false;
        self.saw_engaged = false;
        self.rx_decode.suspend();
        self.tx_decode.end();
        #[cfg(feature = "cw-debug")]
        {
            self.rx_probe = None;
        }
        self.raw_contacts = None;
    }

    pub(super) const fn settings_word(&self) -> u32 {
        self.settings.word()
    }

    pub(super) fn restore_settings(&mut self, word: u32) {
        self.settings = match Settings::decode(word) {
            Some(value) => value,
            None => panic!("validated CW settings rejected"),
        };
        self.cancel();
    }

    /// No session or completion is created by this projection. Reception
    /// can remain in CW while its settings or ordinary menus are open.
    pub(super) const fn idle_demand(&self, selected: bool) -> Demand {
        Demand {
            pitch: if selected { self.settings.pitch } else { 0 },
            gated: self.settings.gated,
            session: false,
            down: false,
            prepare: false,
            completed: false,
        }
    }

    #[cfg(feature = "cw-debug")]
    pub(super) fn observe_contacts(&mut self, contacts: Option<(bool, bool)>) {
        self.raw_contacts = contacts;
    }

    pub(super) fn home_view(&self) -> super::home::CwView<'_> {
        super::home::CwView {

            rx_line: self.rx_decode.display_line(),
            tx_line: self.tx_decode.display_line(),
            rx_live: self.rx_decode.live(
                crate::mcu::tick::snapshot(), self.settings.wpm,
            ),
            tx_live: self.tx_decode.live(
                crate::mcu::tick::snapshot(), self.settings.wpm,
            ),
            input: self.settings.input_name(),
            usb: self.settings.usb_input(),
            testing: self.settings.testing(),
            wpm: self.settings.wpm,
            pitch: self.settings.pitch,
            hang: self.settings.hang,
            breakin: 0,
            key: self.settings.mode_name(),
            reverse: self.settings.reverse,
            gated: self.settings.gated,
            qsk: self.settings.qsk,
            qualified: self.qualified,
            contacts: self.raw_contacts,
        }
    }

    pub(super) const fn usb_input(&self) -> bool {
        self.settings.usb_input()
    }

    pub(super) const fn testing(&self) -> bool {
        self.settings.testing()
    }

    #[cfg(feature = "cw-debug")]
    pub(super) fn open(&mut self) {
        self.cancel();
    }

    /// The observation page may clear text without changing saved choices,
    /// receiver thresholds, or transmitter release qualification.
    pub(super) fn clear_text(&mut self) {
        self.rx_decode.clear();
        self.tx_decode.clear();
    }
    #[inline(never)]
    pub(super) fn drive(
        &mut self,
        now: u32,
        contacts: Option<(bool, bool)>,
        engaged: bool,
        ready: bool,
        mark_ready: bool,
        prepare_ticks: u32,
        error: u32,
    ) -> Demand {
        let idle = Demand {
            pitch: self.settings.pitch, gated: self.settings.gated,
            session: false, down: false, prepare: false, completed: false,
        };

        if error != 0 || contacts.is_none()
            || (self.polled && now.wrapping_sub(self.last_poll) > 20)
            || (self.requested && self.saw_engaged && !engaged)
        {
            self.cancel();
            self.polled = true;
            self.last_poll = now;
            return idle;
        }

        self.polled = true;
        self.last_poll = now;
        let (raw_ptt, raw_side) = contacts.unwrap();
        self.raw_contacts = Some((raw_ptt, raw_side));

        // Track actual selected contacts, not the synthetic session request.
        // Hang time can establish release while TX still owns the chip.
        // An unselected held contact also prevents retaining qualification.
        if raw_ptt || raw_side || (!self.qualified && engaged) {
            self.releasing = false;
        } else if !self.releasing {
            self.releasing = true;
            self.release_at = now;
        }
        let released = self.releasing
            && now.wrapping_sub(self.release_at) >= 100;

        if !self.qualified {
            if released {
                self.qualified = true;
            }

            self.ptt.sample(now, false);
            self.side.sample(now, false);
            return idle;
        }

        let ptt = self.ptt.sample(now, raw_ptt);
        let side = self.side.sample(now, raw_side);
        let keys = self.settings.contacts(ptt, side);

        // Contact display and release qualification remain operational,
        // but this source cannot create even a transmitter setup request.
        if self.settings.testing() {
            return idle;
        }

        if !self.requested {
            if keys == 0 || engaged {
                return idle;
            }

            self.requested = true;
            #[cfg(feature = "cw-debug")]
            {
                self.request_at = now;
            }
            self.lead.arm(now);
            self.pending = keys;
            self.ready = false;
            self.saw_engaged = false;
            self.keyer = Keyer::new();
            self.last_activity = now;
        }

        self.saw_engaged |= engaged;

        let straight = if self.settings.mode < 2 {
            self.lead.poll(now, keys != 0, ready) as u8
        } else {
            0
        };

        if !ready {
            self.pending |= keys;
            self.last_activity = now;
            return Demand {
                pitch: self.settings.pitch, gated: self.settings.gated,
                session: true, down: false, prepare: true, completed: false,
            };
        }

        let input = if !self.ready {
            #[cfg(feature = "cw-debug")]
            {
                self.setup_ticks = Some(now.wrapping_sub(self.request_at));
            }
            self.ready = true;
            let first = if self.settings.mode < 2 {
                straight
            } else {
                keys | self.pending
            };
            self.pending = 0;
            first
        } else if self.settings.mode < 2 {
            straight
        } else {
            keys
        };

        let down = self.keyer.poll_ready(now, input, self.settings, mark_ready);
        if keys != 0 || down || self.keyer.busy() {
            self.last_activity = now;
        }

        if keys == 0 && !self.keyer.busy()
            && now.wrapping_sub(self.last_activity) >= self.settings.hang as u32
        {
            let release_at = self.release_at;
            self.cancel();

            if released {
                // Both contacts are physically released. Preserve that
                // completed observation, not keyer memory or a live session.
                // Retaining poll continuity still rejects a later service gap.
                self.qualified = true;
                self.releasing = true;
                self.release_at = release_at;
                self.polled = true;
                self.last_poll = now;
            }

            return Demand { completed: released, ..idle };
        }

        Demand {
            pitch: self.settings.pitch, gated: self.settings.gated,
            session: true, down,
            prepare: down || self.keyer.prepare_tx(now, input, prepare_ticks),
            completed: false,
        }
    }

    /// Called after the RF owners have progressed. Repeated RX timestamps
    /// cannot advance Morse timing; unavailable RX discards partial input.
    // Borrow both decoder owners through one foreground observation entry.
    #[inline(never)]
    pub(super) fn observe(
        &mut self,
        now: u32,
        receive: Option<(u32, bool)>,
        #[cfg(feature = "cw-debug")]
        probe: Option<crate::cw_decode::Probe>,
        transmit: TxView,
        ready: bool,
    ) {
        #[cfg(feature = "cw-debug")]
        {
            self.rx_probe = if transmit.active || self.requested { None } else { probe };
        }

        if transmit.active || self.requested {
            self.rx_decode.suspend();
        } else if let Some((tick, mark)) = receive {
            self.rx_decode.poll(tick, mark, self.settings.wpm);
        } else {
            self.rx_decode.suspend();
        }

        if ready && transmit.error == 0 {
            self.tx_decode.poll(now, transmit.keyed, self.settings.wpm);
        } else {
            self.tx_decode.end();
        }
    }

    /// Read-only input diagnostics, separate from the native settings page.
    /// No decoded history or editable setting is drawn here.
    #[cfg(feature = "cw-debug")]
    pub(super) fn render(
        &self, frame: &mut Frame, frequency: u32, tx: TxView, rx: RxView,
    ) {
        frame.clear();
        frame.text(2, 0, b"CW INPUTS");
        frame.fill(0, 8, 128, 1, true);
        frame.text_size(2, 11, self.settings.input_name(), Size::Small);

        let contacts = match self.raw_contacts {
            Some((first, second)) => [
                if self.settings.usb_input() { b'U' } else { b'L' },
                b':', b'0' + first as u8, b'0' + second as u8,
            ],
            None => [
                if self.settings.usb_input() { b'U' } else { b'L' },
                b':', b'?', b'?',
            ],
        };
        frame.text_size(46, 11, &contacts, Size::Small);
        frame.text_size(76, 11, b"NO KEYING", Size::Small);
        widgets::frequency(frame, 2, 20, frequency, Size::Small);
        frame.text_size(46, 20, b"START", Size::Small);
        match self.setup_ticks {
            Some(ticks) => {
                widgets::number(frame, 70, 20, ticks.min(99_999), Size::Small);
                frame.text_size(94, 20, b"TICKS", Size::Small);
            }
            None => frame.text_size(70, 20, b"--", Size::Small),
        }

        if let Some(probe) = self.rx_probe {
            frame.text_size(2, 29, b"R", Size::Small);
            widgets::number(frame, 10, 29, probe.rssi, Size::Small);
            frame.text_size(30, 29, b"O", Size::Small);
            widgets::number(frame, 38, 29, probe.open, Size::Small);
            frame.text_size(58, 29, b"C", Size::Small);
            widgets::number(frame, 66, 29, probe.close, Size::Small);
            frame.text_size(
                94, 29, if probe.down { b"MARK" } else { b"SPACE" },
                Size::Small,
            );

            frame.text_size(2, 38, b"M", Size::Small);
            widgets::number(frame, 10, 38, probe.mark_ticks.min(99_999), Size::Small);
            frame.text_size(38, 38, b"S", Size::Small);
            widgets::number(frame, 46, 38, probe.space_ticks.min(99_999), Size::Small);
            frame.text_size(74, 38, b"G", Size::Small);
            widgets::number(frame, 82, 38, probe.max_gap.min(99_999), Size::Small);
        } else {
            frame.text_size(2, 31, b"NO CW RECEIVE OBSERVATION", Size::Small);
        }

        if tx.error != 0 || rx.error != 0 {
            frame.text_size(2, 48, b"ERR", Size::Small);
            frame.hex(22, 47, if tx.error != 0 { tx.error } else { rx.error });
        } else {
            frame.text_size(2, 48, b"RAW RSSI / M S G IN TICKS", Size::Small);
        }
        frame.text_size(2, 58, b"0 CLR TEXT / MENU / EXIT", Size::Small);
    }
}
