//! Foreground channel exchange over the selected command endpoint.
//!
//! Stage runs before input dispatch; Complete runs afterward and before
//! autosave scheduling. Each call handles at most one staged import. Stage
//! can also start one requested export in the endpoint's stable TX buffer.
//! Neither task owns another channel-sized array or consumes transport bytes.
//!
//! The caller supplies audio eligibility, including UI-feedback recovery.
//! The tasks separately exclude engaged TX and busy journal ownership.
//! Import staging and application also require raw and filtered PTT release.
//! Export reads the bank without granting any RF or storage permission.
//!
//! Semantic validation precedes receiver suspension. Invalid input cannot
//! park RF or consume release qualification. A user decision is taken only
//! after application eligibility succeeds, so held PTT cannot lose MENU/EXIT.
//!
//! Panel owns RAM mutation and dirty tracking. The endpoint retains the
//! immutable staged payload and its schema until completion releases it.
//! A staging acknowledgement is not RAM application or committed saving.

use super::panel::Panel;
use crate::drivers::{
    bk4829::{receive::Receiver, transmit::Transmitter},
    keyboard::Input,
    uart::Tx,
};
use crate::storage::journal::Journal;

#[inline(never)]
pub(super) fn stage(
    now: u32,
    audio_idle: bool,
    input: &Input,
    uart: &mut Tx,
    panel: &mut Panel,
    receiver: &mut Receiver,
    transmitter: &mut Transmitter,
    journal: &Journal,
) -> bool {
    if !audio_idle || transmitter.engaged() || journal.busy() {
        return false;
    }

    let mut redraw = false;

    if !input.ptt() && !input.raw_ptt() {
        let version = uart.import_version();
        if let Some(bytes) = uart.import_request() {
            let accepted = panel.prepare_import(bytes, version, now);

            if accepted {
                transmitter.disarm();
                receiver.suspend_for_tx();
            }

            uart.acknowledge_import(accepted);
            redraw = true;
        }
    }

    if uart.export_requested() {
        uart.export_channels(now, |bytes| panel.export_channels(bytes));
    }

    redraw
}

#[inline(never)]
pub(super) fn complete(
    audio_idle: bool,
    input: &Input,
    uart: &mut Tx,
    panel: &mut Panel,
    transmitter: &Transmitter,
    journal: &Journal,
) -> bool {
    if !audio_idle || transmitter.engaged() || journal.busy()
        || input.ptt() || input.raw_ptt()
    {
        return false;
    }

    if let Some(accept) = panel.take_import_decision() {
        let version = uart.import_version();
        if let Some(bytes) = uart.import_data() {
            panel.finish_import(bytes, version, accept);
        }
        uart.finish_import();
        true
    } else {
        false
    }
}