//! Foreground observation after display composition.
//!
//! Driver snapshots are published in foreground-owner order. Each optional
//! publisher decides whether its diagnostic record exists in this composition.
//! UART publication and enabled TX-meter sampling are operational consumers,
//! so the entire task is not controlled by the debug-tools feature.
//!
//! The meter reads REG_64 at most once per accepted sampling interval, only
//! during voice TX and when visualization is selected. It writes no audio or
//! RF control register. Screens consume the published observation instead
//! of accessing the chip during composition.
//!
//! Mandatory stack checking, watchdog service, and final timing observation
//! remain explicit in app::run after this task. Diagnostic publication cannot
//! substitute for any of those protections.

use super::Runtime;
use crate::drivers::{
    adc::Battery,
    bk4829::{meter, receive::Receiver, transmit::Transmitter},
    keyboard::Input,
    st7565::Display,
    uart::Tx,
};
use crate::storage::{journal::Journal, Memory};

#[inline(never)]
pub(super) fn publish(
    now: u32,
    rendered_tick: u32,
    state: &mut Runtime,
    display: &Display,
    input: &Input,
    battery: &Battery,
    memory: &Memory,
    journal: &Journal,
    receiver: &Receiver,
    transmitter: &Transmitter,
    uart: &Tx,
    visualize_tx: bool,
) {
    display.publish(now, rendered_tick);
    input.publish(now);
    battery.publish(now);
    memory.publish(now);
    journal.publish(now);
    receiver.publish(now);
    transmitter.publish(now);
    meter::sample(now, transmitter.voice_active() && visualize_tx);
    uart.publish(now);

    state.display_phase = display.phase();
    let [frames, bytes, failures, dropped] = uart.counters();
    state.uart_frames = frames;
    state.uart_bytes = bytes;
    state.uart_failures = failures;
    state.uart_drops = dropped;

    super::publish(*state);
}