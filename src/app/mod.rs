//! Ordered foreground dispatch over concrete radio and storage owners.
//!
//! Each observed tick services UART input and battery acquisition. Memory
//! and journal progress require no engaged transmitter. Loaded settings,
//! boot completion, channel staging, user input, confirmed import, autosave,
//! and file actions precede the RF/audio task.
//!
//! RF service chooses one owner and publishes its expected control state
//! to the watchdog guard. Display preparation advances the LCD and captures
//! only a stable completed frame. Service and heartbeat events precede new
//! frame composition. At most eight queued events are consumed per pass.
//!
//! The final observation task publishes driver snapshots and optionally
//! samples the TX amplitude indicator. Mandatory stack checking and foreground
//! watchdog service remain explicit after it. Timing capture, when compiled,
//! samples the end only after those protections have run.
//!
//! Service and heartbeat use independent 10-tick and 1000-tick deadlines.
//! Delayed occurrences are counted instead of replayed. TRAKT_RUNTIME exists
//! only with debug-tools and contains observed tick gaps, not WCET.

pub(crate) mod bench;
mod boot;
pub(crate) mod time;
mod events;
mod channel;
mod channels;
mod changes;
pub(crate) mod preferences;
pub(crate) mod appearance;
mod feedback;
mod tasks;
mod exchange;
mod input_task;
mod dual;
pub(crate) mod ptt_check;
mod presentation;
mod observations;
mod diagnostic_screens;
mod scan_range;
mod tx_route;
mod tx_signal;
#[cfg(feature = "spectrum")]
mod spectrum;
mod menu;
mod operation;
mod panel;
mod power;
mod screens;
pub(crate) mod squelch;
pub(crate) mod sound;
pub(crate) mod tx_policy;
pub(crate) mod tx_options;
pub(crate) mod tx_notice;



use crate::drivers::{
    adc::Battery, bk4829::receive::Receiver,
    keyboard::Input, st7565::Display, uart::Tx,
};
use crate::mcu::{self, clock::Clocks};
use crate::{startup, ui};
use crate::storage::journal::Journal;
use crate::drivers::bk4829::transmit::Transmitter;
use events::{Event, Kind, Queue};
use time::Periodic;

#[repr(C)]
#[derive(Clone, Copy)]
struct Runtime {
    signature: u32,
    tick: u32,
    passes: u32,
    services: u32,
    heartbeats: u32,
    missed: u32,
    queue_drops: u32,
    uart_frames: u32,
    uart_bytes: u32,
    uart_failures: u32,
    uart_drops: u32,
    hclk: u32,
    pclk: u32,
    max_tick_gap: u32,
    last_event: u32,
    display_phase: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_RUNTIME: Runtime = unsafe { core::mem::zeroed() };

fn publish(mut state: Runtime) {
    if !cfg!(feature = "debug-tools") {
        return;
    }

    state.signature = 0;
    crate::observation::publish!(TRAKT_RUNTIME, state, 0x544E_5254);
}

pub(crate) fn run(clocks: Clocks, mut uart: Tx, mut guard: bench::Guard) -> ! {
    let mut state = Runtime {
        hclk: clocks.hclk,
        pclk: clocks.pclk,
        ..unsafe { core::mem::zeroed() }
    };
    let mut queue = Queue::new();
    let mut service = Periodic::new(0, 10);
    let mut heartbeat = Periodic::new(0, 1_000);
    let mut previous = 0;
    let mut presentation = presentation::Presenter::new(0, 250);

    let mut input = unsafe { Input::initialize() };
    let mut battery = match unsafe { Battery::initialize(clocks.pclk) } {
        Some(battery) => battery,
        None => startup::stop(0x1009),
    };
    let mut memory = match unsafe { crate::storage::Memory::initialize(clocks.pclk) } {
        Some(memory) => memory,
        None => startup::stop(0x100A),
    };
    let mut journal = unsafe { Journal::new() };
    let mut panel = unsafe { panel::Panel::new() };
    let mut receiver = Receiver::new();
    let mut transmitter = Transmitter::new();
    let mut diagnostics = crate::debug::Monitor::new();


    let mut boot = boot::Boot::new();
    let mut boot_sound = crate::drivers::bk4829::sound::Local::new();
    let mut feedback = feedback::Feedback::new();
    let frame = unsafe { ui::frame() };
    boot.render(frame);
    let mut display = match unsafe { Display::initialize(clocks.pclk) } {
        Some(display) => display,
        None => startup::stop(0x1008),
    };

    publish(state);
    uart.queue(0, [0, clocks.hclk, clocks.pclk, 0, 0]);
    unsafe { mcu::enable_interrupts() };

    loop {
        let now = mcu::tick::wait(previous);
        state.max_tick_gap = state.max_tick_gap.max(now.wrapping_sub(previous));
        previous = now;
        state.tick = now;
        state.passes = state.passes.wrapping_add(1);
        let mut redraw_pending = false;
        uart.pump(now);
        battery.pump(now);
        if !transmitter.engaged() {
            memory.pump(now);
            journal.pump(&mut memory, now);
        }

        let boot_finished = boot.poll(
            now, display.ready(), memory.view(), journal.busy(), battery.reading(now),
        );
        if boot_finished {
            redraw_pending = true;
        }

        redraw_pending |= tasks::load(&mut journal, &mut panel, &mut transmitter);

        if boot_finished && panel.sound_settings().boot {
            receiver.suspend_for_tx();
            if let Err(error) = boot_sound.start(now, sound::boot()) {
                startup::stop(error);
            }
        }

        if journal.take_saved() {
            panel.saved();
            redraw_pending = true;
        }

        redraw_pending |= exchange::stage(
            now,
            !boot.active() && !boot_sound.active() && !feedback.busy(),
            &input, &mut uart, &mut panel, &mut receiver,
            &mut transmitter, &journal,
        );

        redraw_pending |= input_task::poll(
            now,
            &mut input,
            &mut panel,
            input_task::Gate {
                boot: boot.active(),
                boot_audio: boot_sound.active(),
                storage: journal.busy(),
                transmitting: transmitter.engaged(),
            },
        );

        redraw_pending |= exchange::complete(
            !boot.active() && !boot_sound.active() && !feedback.busy(),
            &input, &mut uart, &mut panel, &transmitter, &journal,
        );

        panel.autosave_poll(
            now,
            !boot.active()
                && !boot_sound.active()
                && !feedback.busy()
                && !transmitter.engaged()
                && !receiver.view().signal
                && !input.ptt()
                && !input.raw_ptt(),
            journal.busy(),
            journal.view().error,
        );

        redraw_pending |= tasks::files(
            now, &mut panel, &mut journal, &mut transmitter,
        );
        let debug_test = panel.take_test_action();
        if debug_test.is_some() {
            transmitter.disarm();
        }

        tasks::radio(
            now, boot.active(), &mut boot_sound, &mut feedback,
            &mut panel, &input, &battery, &memory, &journal,
            &mut receiver, &mut transmitter, &mut guard,
        );
        if let Some(test) = debug_test {
            if journal.busy() || transmitter.engaged() || receiver.owns_rf()
                || boot_sound.active() || feedback.busy()
                || input.ptt() || input.raw_ptt()
            {
                panel.refuse_test();
                redraw_pending = true;
            } else if cfg!(feature = "debug-tools") {
                crate::debug::execute(test);
            }
        }

        presentation.prepare(
            now, redraw_pending, &panel, &mut display,
            frame, &mut uart, &transmitter,
        );

        let due = service.poll(now);
        if due != 0 {
            state.missed = state.missed.saturating_add(due - 1);
            let _ = queue.push(Event { kind: Kind::Service, tick: now });
        }
        let due = heartbeat.poll(now);
        if due != 0 {
            state.missed = state.missed.saturating_add(due - 1);
            let _ = queue.push(Event { kind: Kind::Heartbeat, tick: now });
        }

        for _ in 0..8 {
            let event = match queue.pop() {
                Some(event) => event,
                None => break,
            };
            state.last_event = event.tick;
            match event.kind {
                Kind::Service => {
                    state.services = state.services.wrapping_add(1);
                }
                Kind::Heartbeat => {
                    match mcu::clock::inspect() {
                        Ok(current) if current.hclk == clocks.hclk
                            && current.pclk == clocks.pclk => {}
                        _ => startup::stop(0x1006),
                    }
                    state.heartbeats = state.heartbeats.wrapping_add(1);
                    uart.queue(now, [
                        now,
                        clocks.hclk,
                        clocks.pclk,
                        state.missed,
                        queue.dropped,
                    ]);
                }
            }
        }

        presentation.render(
            ui::Snapshot {
                tick: now,
                hclk: clocks.hclk,
                pclk: clocks.pclk,
                missed: state.missed,
                frames: display.frames(),
            },
            &boot, &panel, frame, &mut display, &input, &battery,
            &memory, &receiver, &journal, &transmitter,
        );

        state.queue_drops = queue.dropped;
        observations::publish(
            now, presentation.rendered_tick(), &mut state,
            &display, &input, &battery, &memory, &journal,
            &receiver, &transmitter, &uart,
            panel.appearance().tx_visual != 0,
        );
        diagnostics.poll(now);
        guard.service(now);
        if cfg!(feature = "debug-tools") {
            crate::debug::timing::record(now, mcu::tick::snapshot());
        }
    }
}