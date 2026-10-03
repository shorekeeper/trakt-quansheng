//! One foreground input poll with explicit UI dispatch admission.
//!
//! Hardware filters are polled even when UI dispatch is blocked, so PTT
//! continues to update independently of navigation. Blocked events are
//! discarded rather than replayed onto a later screen.
//!
//! Boot presentation, boot audio, and busy storage exclude panel events.
//! An engaged transmitter admits only a fresh EXIT Down. Panel changes its
//! request; the following RF task performs transmitter cancellation.
//! PTT filtering and admission do not depend on a panel key handler.
//!
//! UI feedback is not a dispatch lock. Keys can change presentation or cause
//! playback cancellation through the RF/audio task. Any observed event asks
//! for redraw, including an event that was not admitted to the panel.

use super::panel::Panel;
use crate::drivers::keyboard::{Edge, Event, Input, Key};

#[derive(Clone, Copy)]
pub(super) struct Gate {
    pub(super) boot: bool,
    pub(super) boot_audio: bool,
    pub(super) storage: bool,
    pub(super) transmitting: bool,
}

impl Gate {
    const fn admits(self, event: Event) -> bool {
        !self.boot && !self.boot_audio && !self.storage
            && (!self.transmitting
                || (matches!(event.key, Key::Exit)
                    && matches!(event.edge, Edge::Down)))
    }
}

/// Polls the existing four-event input batch exactly once. The caller
/// snapshots admission before this call and services RF after dispatch.
#[inline(never)]
pub(super) fn poll(
    now: u32,
    input: &mut Input,
    panel: &mut Panel,
    gate: Gate,
) -> bool {
    let mut redraw = false;

    for event in input.poll(now).into_iter().flatten() {
        if gate.admits(event) {
            panel.handle(event);
        }
        redraw = true;
    }

    redraw
}

const _: () = {
    let keys = [
        Key::None, Key::Menu, Key::Up, Key::Down, Key::Exit,
        Key::One, Key::Two, Key::Three, Key::Four, Key::Five,
        Key::Six, Key::Seven, Key::Eight, Key::Nine,
        Key::Star, Key::Zero, Key::F, Key::Side1, Key::Side2, Key::Ptt,
    ];
    let edges = [Edge::Down, Edge::Up, Edge::Hold, Edge::Repeat];

    let mut mask = 0;
    while mask < 16 {
        let gate = Gate {
            boot: mask & 1 != 0,
            boot_audio: mask & 2 != 0,
            storage: mask & 4 != 0,
            transmitting: mask & 8 != 0,
        };

        let mut key = 0;
        while key < keys.len() {
            let mut edge = 0;
            while edge < edges.len() {
                let event = Event { key: keys[key], edge: edges[edge] };
                let expected = mask == 0
                    || (mask == 8
                        && matches!(event.key, Key::Exit)
                        && matches!(event.edge, Edge::Down));

                assert!(gate.admits(event) == expected);
                edge += 1;
            }
            key += 1;
        }
        mask += 1;
    }
};