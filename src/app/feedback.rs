//! Short local UI cues under exclusive RF ownership.
//!
//! At most one cue is pending. More important action feedback may replace
//! navigation feedback. Temporary ineligibility retains the cue until its
//! 250-tick expiry; expiry or a disabled selection discards it. Retuning
//! therefore need not lose feedback, but old cues cannot replay indefinitely.
//! No key repeat produces a cue.
//!
//! A cue starts only on an idle or settled, silent receiver. Carrier audio,
//! monitor, scanning, Spectrum, storage work, and TX exclude playback.
//! PTT cancels playback immediately. Receiver restoration completes before
//! normal TX dispatch resumes, avoiding a PTT error caused by beep recovery.
//!
//! Local playback temporarily parks RF. The existing receive calibration
//! and profile are retained; restoration uses the ordinary tune sequence
//! rather than rebuilding all presets. SQL starts with fresh history.
//! A failed local transition uses the common RF-off trap.
//!
//! Beeps do not authorize TX and are independent of Roger and permit audio.

use crate::app::{
    sound::{Note, Settings as SoundSettings},
    time::reached,
};
use crate::drivers::bk4829::{
    receive::{Phase, Receiver, Request, View},
    sound::Local,
};
use crate::startup;

#[derive(Clone, Copy)]
pub(super) enum Cue {
    Key,
    Enter,
    Back,
    Apply,
    Delete,
    Saved,
    Reject,
}

const KEY: [Note; 1] = [
    Note { hz: 1050, ticks: 24 },
];

const ENTER: [Note; 1] = [
    Note { hz: 1250, ticks: 35 },
];

const BACK: [Note; 1] = [
    Note { hz: 780, ticks: 35 },
];

const APPLY: [Note; 3] = [
    Note { hz: 1300, ticks: 25 },
    Note { hz: 0, ticks: 8 },
    Note { hz: 1650, ticks: 35 },
];

const DELETE: [Note; 3] = [
    Note { hz: 550, ticks: 30 },
    Note { hz: 0, ticks: 10 },
    Note { hz: 400, ticks: 40 },
];

const SAVED: [Note; 3] = [
    Note { hz: 1200, ticks: 30 },
    Note { hz: 0, ticks: 10 },
    Note { hz: 1600, ticks: 40 },
];

const REJECT: [Note; 3] = [
    Note { hz: 450, ticks: 50 },
    Note { hz: 0, ticks: 20 },
    Note { hz: 450, ticks: 50 },
];

impl Cue {
    const fn priority(self) -> u8 {
        match self {
            Self::Key => 0,
            Self::Enter | Self::Back => 1,
            Self::Apply | Self::Delete => 2,
            Self::Saved | Self::Reject => 3,
        }
    }

    const fn notes(self, mode: u8) -> &'static [Note] {
        if mode == 0 {
            return &[];
        }

        // Every enabled UI mode keeps rejection acoustically distinct.
        // Off still suppresses this cue; RF ownership gates remain unchanged.
        if mode == 1 && !matches!(self, Self::Reject) {
            return &KEY;
        }

        if mode == 2 && matches!(self, Self::Key) {
            return &[];
        }

        match self {
            Self::Key => &KEY,
            Self::Enter => &ENTER,
            Self::Back => &BACK,
            Self::Apply => &APPLY,
            Self::Delete => &DELETE,
            Self::Saved => &SAVED,
            Self::Reject => &REJECT,
        }
    }
}

pub(super) struct Feedback {
    pending: Option<Cue>,
    expires: u32,
    local: Local,
    restore_rx: bool,
    recovering: bool,
    recovery_deadline: u32,
}

impl Feedback {
    pub(super) const fn new() -> Self {
        Self {
            pending: None,
            expires: 0,
            local: Local::new(),
            restore_rx: false,
            recovering: false,
            recovery_deadline: 0,
        }
    }

    pub(super) fn offer(&mut self, cue: Cue, now: u32, mode: u8) {
        if cue.notes(mode).is_empty() {
            return;
        }

        let replace = self.pending.map_or(true, |pending| {
            cue.priority() >= pending.priority()
        });

        if replace {
            self.pending = Some(cue);
            self.expires = now.wrapping_add(250);
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.local.active() || self.recovering
    }

    pub(super) fn local_active(&self) -> bool {
        self.local.active()
    }

    pub(super) fn controls(&self) -> Option<[u16; 5]> {
        self.local.controls()
    }

    pub(super) fn try_start(
        &mut self,
        now: u32,
        mode: u8,
        eligible: bool,
        request: Request,
        receiver: &mut Receiver,
    ) {
        if self.busy() {
            return;
        }

        let cue = match self.pending {
            Some(cue) => cue,
            None => return,
        };

        if reached(now, self.expires) {
            self.pending = None;
            return;
        }

        let notes = cue.notes(mode);

        if notes.is_empty() {
            self.pending = None;
            return;
        }

        if !eligible {
            return;
        }

        let state = receiver.view();
        let settled = matches!(state.phase, Phase::Receive)
            && state.frequency == request.frequency
            && state.narrow == request.narrow
            && state.am == request.am
            && state.rx_signal == request.rx_signal;

        if !matches!(state.phase, Phase::Off) && !settled {
            return;
        }

        if state.audio || state.signal || request.monitor {
            return;
        }

        self.pending = None;
        self.restore_rx = settled && request.enabled;
        receiver.suspend_for_tx();

        if let Err(error) = self.local.start_gain(now, notes, 14) {
            startup::stop(error);
        }
    }

    fn restore(&mut self, now: u32, request: Request, receiver: &mut Receiver) {
        if self.restore_rx && request.enabled {
            if let Err(error) = receiver.resume_after_feedback(now) {
                startup::stop(error);
            }

            self.recovering = true;
            self.recovery_deadline = now.wrapping_add(500);
        } else {
            self.recovering = false;
        }

        self.restore_rx = false;
    }

    pub(super) fn pump(
        &mut self,
        now: u32,
        cancel: bool,
        request: Request,
        receiver: &mut Receiver,
    ) {
        if !self.local.active() {
            return;
        }

        if cancel {
            self.local.cancel();
        } else if let Err(error) = self.local.pump(now) {
            startup::stop(error);
        }

        if !self.local.active() {
            self.restore(now, request, receiver);
        }
    }

    pub(super) fn observe_receiver(&mut self, now: u32, receiver: View) {
        if !self.recovering {
            return;
        }

        if matches!(receiver.phase, Phase::Receive | Phase::Off | Phase::Failed) {
            self.recovering = false;
        } else if reached(now, self.recovery_deadline) {
            startup::stop(0x7504);
        }
    }
}

const _: () = {
    assert!(Cue::Key.notes(0).is_empty());
    assert!(Cue::Key.notes(2).is_empty());
    assert!(Cue::Apply.notes(2).len() == 3);
    assert!(Cue::Saved.notes(1).len() == 1);
    assert!(Cue::Reject.notes(0).is_empty());
    assert!(Cue::Reject.notes(1).len() == 3);
    assert!(Cue::Reject.notes(2).len() == 3);
    assert!(Cue::Reject.notes(3).len() == 3);
    assert!(Cue::Saved.priority() > Cue::Key.priority());
    assert!(Cue::Delete.priority() > Cue::Enter.priority());

    let owner = Feedback::new();
    assert!(!owner.recovering);
    assert!(owner.pending.is_none());

    // Sound preference defaults remain separate from UI feedback.
    assert!(SoundSettings::new().roger == 0);
    assert!(SoundSettings::new().permit == 0);
};