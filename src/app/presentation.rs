//! Display scheduling over the application's single framebuffer.
//!
//! Prepare advances the LCD, applies backlight state, captures a completed
//! frame when requested by UART, and accumulates redraw requests. Render
//! runs after foreground service events. A capture copies the completed
//! frame before the application draws its replacement.
//!
//! Pending redraw survives a busy LCD and clears only when display.request
//! accepts the newly composed frame. No drawing occurs before display.ready.
//! Presenter stores timing and admission state, not another framebuffer.
//!
//! Periodic refresh follows the selected display interval. Voice-TX
//! visualization additionally requests redraw after fifty observed ticks
//! since the last accepted frame. UI redraw requests are not rate-limited
//! by that meter interval. Delayed periods are coalesced, not replayed.
//! A displayed TX error appearing or expiring also requests composition,
//! independently of the selected periodic refresh. A busy LCD still drains
//! its current frame before the pending replacement can be composed.
//!
//! Source objects are borrowed only during composition. Rendering does not
//! configure RF, change settings, or service watchdogs. Scheduling deadlines
//! are observed ticks, not measurements of display or RF latency.

use super::{boot::Boot, panel::Panel, time::Periodic};
use crate::drivers::{
    adc::Battery,
    bk4829::{receive::Receiver, transmit::Transmitter},
    keyboard::Input,
    st7565::Display,
    uart::Tx,
};
use crate::storage::{journal::Journal, Memory};
use crate::ui::{Frame, Snapshot};

pub(super) struct Presenter {
    repaint: Periodic,
    period: u32,
    pending: bool,
    rendered: u32,
    tx_error: u32,
}

impl Presenter {
    pub(super) const fn new(now: u32, period: u32) -> Self {
        Self {
            repaint: Periodic::new(now, period),
            period,
            pending: false,
            rendered: now,
            tx_error: 0,
        }
    }

    const fn configure(&mut self, now: u32, period: u32) {
        if period != self.period {
            self.period = period;
            self.repaint = Periodic::new(now, period);
        }
    }

    const fn observe(&mut self, now: u32, requested: bool, voice_meter: bool) {
        // Always advance the periodic deadline, even with a pending redraw.
        let periodic = self.repaint.poll(now) != 0;
        if requested || periodic
            || (voice_meter && now.wrapping_sub(self.rendered) >= 50)
        {
            self.pending = true;
        }
    }

    const fn accepted(&mut self, now: u32) {
        self.rendered = now;
        self.pending = false;
    }

    pub(super) const fn rendered_tick(&self) -> u32 {
        self.rendered
    }

    #[inline(never)]
    pub(super) fn prepare(
        &mut self,
        now: u32,
        requested: bool,
        panel: &Panel,
        display: &mut Display,
        frame: &Frame,
        uart: &mut Tx,
        transmitter: &Transmitter,
    ) {
        self.configure(now, panel.period);

        display.pump(now, frame);
        display.backlight(panel.light);

        if display.ready() && !transmitter.engaged() && uart.capture_requested() {
            uart.capture(now, frame);
        }

        let error = transmitter.view().error;
        let notification_changed = error != self.tx_error;
        self.tx_error = error;

        self.observe(
            now,
            requested || notification_changed,
            transmitter.voice_active() && panel.appearance().tx_visual != 0,
        );
    }

    /// Sources are borrowed only while composing a frame. None of their
    /// operational state is changed by screen rendering.
    #[inline(never)]
    pub(super) fn render(
        &mut self,
        state: Snapshot,
        boot: &Boot,
        panel: &Panel,
        frame: &mut Frame,
        display: &mut Display,
        input: &Input,
        battery: &Battery,
        memory: &Memory,
        receiver: &Receiver,
        journal: &Journal,
        transmitter: &Transmitter,
    ) {
        if !self.pending || !display.ready() {
            return;
        }

        let now = state.tick;

        if boot.active() {
            boot.render(frame);
        } else {
            panel.render(
                frame,
                state,
                input,
                battery.reading(now),
                memory.view(),
                receiver.view(),
                journal.view(),
                transmitter.view(),
            );
        }

        if display.request() {
            self.accepted(now);
        }
    }
}

const _: () = {
    let mut display = Presenter::new(0, 250);
    display.observe(249, false, false);
    assert!(!display.pending);
    display.observe(250, false, false);
    assert!(display.pending);

    // A busy display cannot consume the pending request.
    display.observe(251, false, false);
    assert!(display.pending && display.rendered_tick() == 0);
    display.accepted(251);
    assert!(!display.pending && display.rendered_tick() == 251);

    display.observe(400, true, false);
    assert!(display.pending);
    display.accepted(400);
    display.observe(499, false, false);
    assert!(!display.pending);
    display.observe(500, false, false);
    assert!(display.pending);
    display.accepted(500);

    display.configure(501, 125);
    display.observe(625, false, false);
    assert!(!display.pending);
    display.observe(626, false, false);
    assert!(display.pending);
    display.accepted(626);

    let mut voice = Presenter::new(0, 500);
    voice.observe(49, false, true);
    assert!(!voice.pending);
    voice.observe(50, false, true);
    assert!(voice.pending);
    voice.accepted(50);
    voice.observe(99, false, true);
    assert!(!voice.pending);
    voice.observe(100, false, true);
    assert!(voice.pending);

    // An ordinary UI request is not rate-limited by the TX meter interval.
    voice.accepted(100);
    voice.observe(101, true, true);
    assert!(voice.pending);
    voice.accepted(101);
    voice.observe(151, false, false);
    assert!(!voice.pending);

    let start = u32::MAX - 20;
    let mut wrapped = Presenter::new(start, 250);
    wrapped.observe(28, false, true);
    assert!(!wrapped.pending);
    wrapped.observe(29, false, true);
    assert!(wrapped.pending);
    wrapped.accepted(29);

    let mut delayed = Presenter::new(0, 250);
    delayed.observe(1000, true, false);
    assert!(delayed.pending);
    delayed.accepted(1000);
    delayed.observe(1001, false, false);
    assert!(!delayed.pending);
    delayed.observe(1250, false, false);
    assert!(delayed.pending);
};