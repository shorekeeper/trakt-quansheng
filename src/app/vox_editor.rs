//! Detached graphical and decimal VOX-threshold editing.
//!
//! The graph and decimal field select the same raw amplitude threshold.
//! Arrows clamp with steps of 1, 16, or 256 counts; F cycles the step.
//! Native menu editing remains available separately. This screen never
//! changes enable or hang, starts TX, or accesses the RF transport.
//!
//! Digits build a detached one-to-five-digit decimal candidate. MENU accepts
//! 1..65535; incomplete here means an empty field, not a fixed-width input.
//! An invalid candidate remains editable. F deletes one digit while numeric
//! input is active. Arrows leave numeric input and adjust the graph draft.
//! EXIT discards the complete threshold draft.
//!
//! The graph starts at zero and uses a power-of-two upper bound above the
//! draft, limited to 2047. It shows the comparator threshold and its cached
//! event state, not a live microphone amplitude meter. Unknown state is
//! explicit until the chip supplies an opening or closing event.
//! Observations make no TX request on this screen. The editor owns scalar
//! draft state only, not a sampler, event consumer, or framebuffer.

use crate::drivers::keyboard::{Edge, Event, Key};
use crate::ui::{font::Size, widgets, Frame};
use crate::vox::Settings;

pub(super) enum Effect {
    Cancel,
    Apply(u16),
}

pub(super) struct Editor {
    draft: Settings,
    step: usize,
    input: u32,
    digits: usize,
    invalid: bool,
}

const STEPS: [u32; 3] = [1, 16, 256];

const fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

const fn upper(threshold: u16) -> u32 {
    let mut value = 64u32;
    while value <= threshold as u32 && value < 2048 {
        value *= 2;
    }
    if value > 2047 { 2047 } else { value }
}

impl Editor {
    pub(super) const fn new(mut settings: Settings) -> Self {
        assert!(settings.valid());
        if settings.threshold > 2047 {
            settings.threshold = 2047;
        }
        Self {
            draft: settings, step: 0, input: 0, digits: 0, invalid: false,
        }
    }

    /// Hardware preview uses the detached arrow-selected draft. Decimal
    /// input does not reprogram the detector before confirmation.
    pub(super) const fn threshold(&self) -> u16 {
        self.draft.threshold
    }

    // Comparator-threshold editing remains a detached runtime operation.
    #[inline(never)]
    pub(super) const fn key(&mut self, event: Event) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }
        let down = matches!(event.edge, Edge::Down);

        if down && matches!(event.key, Key::Exit) {
            return Some(Effect::Cancel);
        }
        if matches!(event.key, Key::Up | Key::Down) {
            self.input = 0;
            self.digits = 0;
            self.invalid = false;
            self.draft.shift_threshold(
                matches!(event.key, Key::Up), STEPS[self.step],
            );
            return None;
        }
        if !down {
            return None;
        }

        if let Some(value) = digit(event.key) {
            if self.digits == 5 {
                self.input = 0;
                self.digits = 0;
            }
            self.input = self.input * 10 + value as u32;
            self.digits += 1;
            self.invalid = false;
        } else if matches!(event.key, Key::F) {
            if self.digits != 0 {
                self.input /= 10;
                self.digits -= 1;
                self.invalid = false;
            } else {
                self.step = (self.step + 1) % STEPS.len();
            }
        } else if matches!(event.key, Key::Menu) {
            if self.digits != 0 {
                if self.input == 0 || self.input > 2047 {
                    self.invalid = true;
                    return None;
                }
                return Some(Effect::Apply(self.input as u16));
            }
            return Some(Effect::Apply(self.draft.threshold));
        }
        None
    }

    // Threshold composition consumes cached observations without RF access.
    #[inline(never)]
    pub(super) fn render(
        &self, frame: &mut Frame, sample: Option<crate::vox::Sample>,
    ) {
        frame.clear();
        frame.text(2, 0, b"VOX THRESHOLD");
        frame.fill(0, 8, 128, 1, true);

        let value = if self.digits != 0 {
            self.input
        } else {
            self.draft.threshold as u32
        };
        widgets::number(frame, 2, 13, value, Size::Large);
        frame.text_size(80, 18, b"RAW CODE", Size::Small);
        frame.text_size(80, 25, b"STEP", Size::Small);
        widgets::number(frame, 100, 25, STEPS[self.step], Size::Small);

        let maximum = upper(self.draft.threshold);
        frame.outline(2, 36, 124, 7, true);
        let position = widgets::extent(
            self.draft.threshold as u32, maximum, 121,
        );
        // This axis locates the selected comparator threshold only.
        // Detector state is displayed as text below. No filled region is
        // drawn because no microphone amplitude was measured during RX.
        frame.fill(3 + position as i16, 36, 1, 7, true);
        widgets::number(frame, 2, 44, 0, Size::Small);
        widgets::number(frame, 102, 44, maximum, Size::Small);

        // Last nonzero VOX event subset in this detector ownership period.
        // Ordinary polls retain it; retuning and detector shutdown clear it.
        if let Some(observation) = sample {
            frame.text_size(30, 44, b"EVT", Size::Small);
            widgets::hex(
                frame, 46, 44, observation.events as u32, Size::Small,
            );
        }

        frame.text_size(2, 51, if self.invalid {
            b"REFUSE: THRESHOLD 1..2047"
        } else if self.digits != 0 {
            b"DECIMAL INPUT / F DELETE"
        } else {
            match sample {
                Some(value) => match value.voice {
                    Some(true) => b"DETECTOR VOICE / F STEP",
                    Some(false) => b"DETECTOR QUIET / F STEP",
                    None => b"WAIT DETECTOR EDGE / F STEP",
                },
                None => b"DETECTOR OFF / F STEP",
            }
        }, Size::Small);
        frame.text_size(2, 58, b"MENU APPLY / EXIT CANCEL", Size::Small);
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    assert!(upper(1) == 64);
    assert!(upper(128) == 256);
    assert!(upper(2047) == 2047);
    assert!(upper(65535) == 2047);

    let original = Settings::new();
    let mut editor = Editor::new(original);
    assert!(editor.key(press(Key::Up)).is_none());
    assert!(editor.draft.threshold == original.threshold + 1);
    assert!(editor.key(press(Key::F)).is_none());
    assert!(editor.step == 1);
    assert!(editor.key(press(Key::Down)).is_none());
    assert!(editor.draft.threshold == original.threshold + 1 - 16);
    assert!(matches!(editor.key(press(Key::Exit)), Some(Effect::Cancel)));
    assert!(original.threshold == 128);

    let mut numeric = Editor::new(original);
    let digits = [Key::Two, Key::Zero, Key::Four, Key::Seven];
    let mut index = 0;
    while index < digits.len() {
        assert!(numeric.key(press(digits[index])).is_none());
        index += 1;
    }
    assert!(matches!(
        numeric.key(press(Key::Menu)), Some(Effect::Apply(2047)),
    ));

    let mut invalid = Editor::new(original);
    index = 0;
    while index < 5 {
        assert!(invalid.key(press(Key::Nine)).is_none());
        index += 1;
    }
    assert!(invalid.key(press(Key::Menu)).is_none());
    assert!(invalid.invalid && invalid.draft.threshold == 128);
    assert!(invalid.key(press(Key::F)).is_none());
    assert!(invalid.input == 9999 && invalid.digits == 4);
    assert!(invalid.key(press(Key::Menu)).is_none());
    assert!(invalid.invalid);
    assert!(invalid.key(press(Key::F)).is_none());
    assert!(matches!(
        invalid.key(press(Key::Menu)), Some(Effect::Apply(999)),
    ));

    let repeated = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(editor.key(repeated).is_none());
    let mut zero = Editor::new(original);
    assert!(zero.key(press(Key::Zero)).is_none());
    assert!(zero.key(press(Key::Menu)).is_none());
    assert!(zero.invalid);
    assert!(zero.key(press(Key::Up)).is_none());
    assert!(!zero.invalid && zero.digits == 0);
};