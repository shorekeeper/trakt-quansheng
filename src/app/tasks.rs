//! Bounded foreground tasks over concrete device owners.
//!
//! Load consumes at most one validated journal result. File service starts
//! at most one requested journal operation. Their separate call boundaries
//! keep large temporary snapshots out of the dispatcher's local lifetime.
//!
//! Radio service selects boot audio, UI feedback, or transmitter/receiver
//! work. Request construction pins DW reception to the selected VFO while
//! PTT or an engaged attempt requires it. TX policy and calibration use the
//! resolved transmit frequency, independently of the RX request.
//!
//! An acquiring transmitter has not admitted TX hardware and still requires
//! a muted receiver pump with carrier SQL enabled. Other engaged TX phases
//! exclude receiver pumping. The resulting owner supplies the watchdog
//! guard's expected register controls.
//!
//! These functions are not threads or independent resource owners. All RF
//! writes occur in the foreground except common non-returning shutdown.
//! Call boundaries do not establish WCET or maximum call-chain stack usage.

use super::{bench::Guard, feedback::Feedback, panel::Panel};
use crate::drivers::{
    adc::Battery,
    bk4829::{
        receive::Receiver,
        sound::Local,
        transmit::{Context as TxContext, Transmitter},
    },
    keyboard::Input,
};
use crate::storage::{journal::{Action, Journal}, Memory};
use crate::startup;

/// Consume one checked Load result before boot audio can inspect settings.
#[inline(never)]
pub(super) fn load(
    journal: &mut Journal,
    panel: &mut Panel,
    transmitter: &mut Transmitter,
) -> bool {
    if let Some(state) = journal.take_loaded() {
        transmitter.disarm();
        if !panel.restore(&state) {
            startup::stop(0x6001);
        }
        true
    } else {
        false
    }
}

/// Start at most one requested settings operation. Save snapshots RAM once;
/// completion and baseline acceptance remain with the journal and panel.
#[inline(never)]
pub(super) fn files(
    now: u32,
    panel: &mut Panel,
    journal: &mut Journal,
    transmitter: &mut Transmitter,
) -> bool {
    if let Some(action) = panel.take_file_action() {
        transmitter.disarm();
        match action {
            Action::Save => journal.save(panel.saved_state(), false, now),
            Action::TestTorn => journal.save(panel.saved_state(), true, now),
            Action::Load => journal.load(now),
            Action::ResetRam => panel.reset_ram(),
        }
        true
    } else {
        false
    }
}

/// Service the current RF/audio owner once. Calibration and input are
/// borrowed read-only; receiver and transmitter state cannot escape this
/// call. The watchdog guard receives the resulting control expectation.
#[inline(never)]
pub(super) fn radio(
    now: u32,
    boot_active: bool,
    boot_sound: &mut Local,
    feedback: &mut Feedback,
    panel: &mut Panel,
    input: &Input,
    battery: &Battery,
    memory: &Memory,
    journal: &Journal,
    receiver: &mut Receiver,
    transmitter: &mut Transmitter,
    guard: &mut Guard,
) {
    panel.prepare_receive(
        input.ptt() || input.raw_ptt() || transmitter.engaged(),
    );
    let mut request = panel.receive_request();
    if journal.busy() || boot_active || boot_sound.active() {
        request.enabled = false;
    }


    let carrier_watch = panel.dual_active()
        && (input.ptt() || input.raw_ptt() || transmitter.acquiring());
    let transmit = panel.transmit_frequency();
    let context = TxContext {
        dual_watch: panel.dual_active(),
        slope: panel.slope_settings(),
        transmit,
        tx_signal: panel.tx_signal(),
        allowed: !boot_active && !boot_sound.active() && panel.tx_scope()
            && !journal.busy() && journal.view().error == 0,
        request,
        receiver: receiver.view(),
        calibration: match transmit {
            Ok(frequency) => memory.tx_profile(frequency, panel.power()),
            Err(error) => Err(error),
        },
        battery: battery.reading(now),
        ptt: input.ptt(),
        raw_ptt: input.raw_ptt(),
        ham: panel.ham_mode(),
        power: panel.power(),
        tot: panel.tot_ticks(),
    };
    let appearance = panel.appearance();

    if let Some(cue) = panel.take_cue() {
        feedback.offer(cue, now, appearance.beeps);
    }

    if !boot_active && !boot_sound.active() && !transmitter.engaged() {
        feedback.try_start(
            now,
            appearance.beeps,
            panel.feedback_scope()
                && !journal.busy()
                && !input.ptt()
                && !input.raw_ptt(),
            request,
            receiver,
        );
    }

    if boot_sound.active() {
        if let Err(error) = boot_sound.pump(now) {
            startup::stop(error);
        }

        guard.receiver_owned(false);
        guard.transmitter_controls(boot_sound.controls());
    } else if feedback.busy() {
        feedback.pump(
            now,
            input.raw_ptt()
                || input.ptt()
                || journal.busy()
                || appearance.beeps == 0
                || !panel.feedback_scope(),
            request,
            receiver,
        );

        if !feedback.local_active() {
            receiver.pump(
                now,
                request,
                memory.view(),
                memory.squelch(request.frequency, request.squelch, request.am),
                input.ptt() || panel.scope_active() || carrier_watch,
                carrier_watch,
            );

            panel.observe(now, receiver.view(), input.ptt() || input.raw_ptt());
            feedback.observe_receiver(now, receiver.view());
        }

        guard.receiver_owned(receiver.owns_rf());
        guard.transmitter_controls(feedback.controls());
    } else {
        transmitter.pump(
            now, context, receiver, panel.sound_settings(),
        );

        if !transmitter.engaged() || transmitter.acquiring() {
            receiver.pump(
                now,
                request,
                memory.view(),
                memory.squelch(request.frequency, request.squelch, request.am),
                input.ptt() || panel.scope_active() || carrier_watch,
                carrier_watch,
            );
            panel.observe(now, receiver.view(), input.ptt() || input.raw_ptt());
        }

        guard.receiver_owned(receiver.owns_rf());
        guard.transmitter_controls(transmitter.controls());
    }
}
