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
//! A DW hop to an unselected CW VFO selects that VFO's beat offset with AUTO
//! gain and an RSSI-gated speaker, so silent dwell does not open noise. The
//! TX context keeps pitch zero there: hopping must not change transmitter
//! PTT history, which follows the selected non-CW VFO.
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
            #[cfg(any(not(feature = "cw"), feature = "cw-debug"))]
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
    input: &mut Input,
    battery: &Battery,
    memory: &Memory,
    journal: &Journal,
    receiver: &mut Receiver,
    transmitter: &mut Transmitter,
    guard: &mut Guard,
) {
    #[cfg(feature = "cw")]
    if input.cw_usb_select(now, panel.cw_usb()) {
        transmitter.disarm();
    }

    let rf_available = !boot_active && !boot_sound.active()
        && !journal.busy() && journal.view().error == 0 && !feedback.busy();
    let automatic_voice = transmitter.vox_active();
    let receiving = receiver.view();
    // CW compositions have neither comparator observations nor automatic
    // voice admission, including when their selected VFO uses FM.
    let vox_request = if cfg!(feature = "cw") {
        false
    } else {
        let sample = if automatic_voice {
            transmitter.vox_sample(now).or_else(|| {
                if transmitter.acquiring() { receiver.vox_sample(now) } else { None }
            })
        } else if transmitter.engaged() {
            None
        } else {
            receiver.vox_sample(now)
        };
        panel.vox_drive(
            now,
            sample,
            rf_available,
            input.ptt() || input.raw_ptt()
                || (transmitter.engaged() && !automatic_voice)
                || (!transmitter.engaged()
                    && (receiving.signal || receiving.audio || receiving.error != 0)),
            automatic_voice && !transmitter.acquiring()
                && !transmitter.voice_active(),
        )
    };

    panel.prepare_receive(
        input.ptt() || input.raw_ptt() || transmitter.engaged() || vox_request,
    );
    let mut request = panel.receive_request();
    if journal.busy() || boot_active || boot_sound.active() {
        request.enabled = false;
    }


    receiver.vox_select(
        if !cfg!(feature = "cw") && rf_available && request.enabled && !request.am
            && !panel.cw_active() && !panel.scope_active()
        {
            panel.vox_threshold()
        } else {
            0
        },
    );

    let carrier_watch = transmitter.acquiring()
        || (panel.dual_active() && (input.ptt() || input.raw_ptt()));
    let transmit = panel.transmit_frequency();

    #[cfg(feature = "cw")]
    let cw = panel.cw_drive(
        now, input, transmitter,
        !boot_active && !boot_sound.active() && !journal.busy() && !feedback.busy(),
    );

    // DW eligibility is evaluated once per pass and reused below.
    #[cfg(feature = "cw")]
    let dual_cw = panel.dual_cw();

    #[cfg(feature = "cw")]
    receiver.cw_select(
        if request.enabled { cw.pitch } else { 0 },
        cw.gated || dual_cw,
    );

    #[cfg(feature = "cw")]
    {
        let controls = panel.cw_controls();
        let acquiring_rf = panel.cw_qsk() && cw.session;
        receiver.cw_options(
            controls.threshold,
            if acquiring_rf || dual_cw { 0 } else { controls.gain },
        );
    }

    let (ptt, raw_ptt) = {
        #[cfg(feature = "cw")]
        {
            if panel.cw_active() {
                (cw.session, cw.session)
            } else {
                (input.ptt(), input.raw_ptt())
            }
        }
        #[cfg(not(feature = "cw"))]
        {
            (input.ptt(), input.raw_ptt())
        }
    };

    #[cfg(feature = "fsk")]
    receiver.fsk_enabled(
        panel.fsk_status() != 0
            && request.enabled && !request.am && request.rx_signal == 0
            && !feedback.busy() && !transmitter.engaged(),
    );

    let mut context = TxContext {
        #[cfg(feature = "cw")]
        cw_pitch: if dual_cw { 0 } else { cw.pitch },
        #[cfg(feature = "cw")]
        cw_down: cw.down,
        #[cfg(feature = "cw")]
        cw_prepare: cw.prepare,
        #[cfg(feature = "cw")]
        cw_complete: cw.completed,
        #[cfg(feature = "cw")]
        cw_qsk: panel.cw_qsk(),
        #[cfg(feature = "cw")]
        cw_volume: panel.cw_controls().volume,
        #[cfg(feature = "cw")]
        cw_breakin: panel.cw_controls().breakin,
        #[cfg(feature = "cw")]
        cw_tail: panel.cw_tail_word(),
        #[cfg(feature = "fsk")]
        fsk_status: panel.fsk_status(),
        #[cfg(feature = "fsk")]
        fsk_message: panel.fsk_message(),
        #[cfg(feature = "fsk")]
        fsk_ack: panel.fsk_ack(),
        #[cfg(feature = "fsk")]
        fsk_ack_allowed: !boot_active && !boot_sound.active()
            && !journal.busy() && journal.view().error == 0
            && panel.fsk_ack_scope(),
        vox_request,
        vox_complete: !cfg!(feature = "cw") && panel.vox_completed(),
        vox_threshold: if cfg!(feature = "cw") { 0 } else { panel.vox_threshold() },
        voice: panel.voice_settings(),
        dual_watch: panel.dual_active(),
        slope: panel.slope_settings(),
        transmit,
        tx_signal: panel.tx_signal(),
        allowed: !boot_active && !boot_sound.active() && panel.tx_scope()
            && !journal.busy() && journal.view().error == 0,
        request,
        receiver: {
            // Selection setters above change pending configuration, not
            // the operational View. No receiver pump has occurred since
            // receiving was captured. Admission validity remains separate.
            let mut view = receiving;
            if vox_request && (!automatic_voice || transmitter.acquiring()) {
                let (valid, occupied) = receiver.vox_admission(now);
                if !valid {
                    view.phase = crate::drivers::bk4829::receive::Phase::Wait;
                }
                view.signal = occupied;
            }
            #[cfg(feature = "cw")]
            {
                let mut view = view;
                if cw.pitch != 0 && cw.session && panel.cw_qsk() {
                    let (valid, occupied) = receiver.cw_admission(now);
                    if !valid {
                        view.phase = crate::drivers::bk4829::receive::Phase::Wait;
                    }

                    // This detached TX-admission snapshot includes immediate
                    // RSSI occupancy. Receiver SQL, decoder thresholds, audio,
                    // and the operational receiver view remain unchanged.
                    view.signal = occupied;
                }
                view
            }
            #[cfg(not(feature = "cw"))]
            { view }
        },
        calibration: match transmit {
            Ok(frequency) => memory.tx_profile(frequency, panel.power()),
            Err(error) => Err(error),
        },
        battery: battery.reading(now),
        ptt,
        raw_ptt,
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
                && !input.raw_ptt()
                && !vox_request,
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
            now, &mut context, receiver, panel.sound_settings(),
        );

        if !transmitter.engaged() || transmitter.acquiring() {
            #[cfg(feature = "cw")]
            if cw.pitch != 0 {
                // The enclosing branch admits only released TX ownership
                // or acquisition, which owns no TX hardware. Both may use
                // the same checked warm receiver restoration.
                receiver.resume_cw(now, request);
            }

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

    if !cfg!(feature = "cw") && transmitter.take_vox_finished() {
        panel.cancel_vox();
    }

    #[cfg(feature = "fsk")]
    {
        let automatic = transmitter.ack_active();
        let ack_result = transmitter.take_ack_result();
        panel.observe_fsk(
            receiver.fsk_view(), receiver.take_link_packet(), transmitter.view(),
            automatic, ack_result,
        );
    }

    #[cfg(feature = "cw")]
    panel.observe_cw(now, receiver, transmitter);
}

