//! Foreground navigation, settings application, and dirty-state ownership.
//!
//! Home requests reception after boot acquisition. Saved HAM and Dual Watch
//! selections are restored by Load. Defaults and Reset RAM select HAM On and
//! DW Off. Runtime monitor, scan progress, and temporary DW receive position
//! are not saved. Channel import preserves independent global settings.
//!
//! Menu navigation does not retune until an accepted action changes a profile.
//! VFO route and signal editors hold detached candidates and mask reception
//! while open. Channel editing and bank mutation disable the operation's
//! requested RX state; cancelling a channel draft does not re-enable it.
//!
//! The baseline contains the entire logical settings snapshot. Input changes
//! reconcile it after dispatch. Scan tuning reconciles only when frequency or
//! slot changes; temporary DW hops never affect it. Successful committed Save
//! and validated Load accept a baseline. Reset RAM and TestTorn do not.
//!
//! No Auto requires explicit saving. Warn offers a Save when leaving changed
//! settings. Auto schedules the ordinary journal operation only when admission
//! permits it. The dirty marker is a comparison result, not storage ownership.
//!
//! F release selects A/B unless Hold toggled monitor. Numeric Down starts
//! frequency input. Detached editors consume their own keys. TX admission
//! requires the Home radio scope and cannot be obtained through a draft,
//! navigation event, import acknowledgement, or a retained live deadline.

use crate::drivers::{
    adc::Reading,
    bk4829::{
        receive::{Request, View},
        transmit::View as TxView,
    },
    keyboard::{Edge, Event, Input, Key},
};
use crate::storage::{
    journal::{Action as FileAction, View as JournalView},
    record::Snapshot as SavedState,
};
use crate::ui::{self, Frame, Snapshot};

use super::{
    changes::Changes,
    channels::{Browser, Effect as ChannelEffect},
    menu::{Action, Effect, Menu, Setting, Values},
    operation::{self, Bank, Mode, Operation},
    screens,
};

#[derive(Clone, Copy)]
enum Screen {
    Home,
    TxRoute,
    TxSignal,
    RxSignal,
    ScanRange,
    #[cfg(feature = "spectrum")]
    Spectrum,
    Import,
    HamWarning,
    Channels,
    Channel,
    Menu,
    Input,
    System,
    Memory,
    Files,
    Help,
    Stacks,
    LastFault,
    Power,
    Radio,
    Runtime,
    Layout,
    About,
    Serial,
}

pub(super) struct Panel {
    screen: Screen,
    last: Option<Event>,
    pub(super) light: bool,
    pub(super) period: u32,
    sounds: super::sound::Settings,
    slope: crate::slope::Settings,
    operation: Operation,
    dual: super::dual::Watch,
    bank: &'static mut Bank,
    appearance: super::appearance::Settings,
    cue: Option<super::feedback::Cue>,
    menu: Menu,
    browser: Browser,
    range_editor: Option<super::scan_range::Editor>,
    route_editor: Option<super::tx_route::Editor>,
    signal_editor: Option<super::tx_signal::Editor>,
    #[cfg(feature = "spectrum")]
    scope: super::spectrum::Scope,
    import_count: u32,
    import_decision: Option<bool>,
    import_ready_at: u32,
    channel_editor: Option<super::channel::Editor>,
    changes: Changes,
    preferences: super::preferences::Preferences,
    autosave: super::preferences::Autosave,
    automatic_save: bool,
    unsaved_warning: bool,
    file_action: Option<FileAction>,
    ram_reset: bool,
    f_held: bool,
    ham: bool,
    power: u8,
    tot: u8,
    warning_stage: u8,
    test_action: Option<crate::debug::Test>,
    test_refused: bool,
}

fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

fn digit(key: Key) -> bool {
    matches!(
        key,
        Key::Zero | Key::One | Key::Two | Key::Three | Key::Four
            | Key::Five | Key::Six | Key::Seven | Key::Eight | Key::Nine
    )
}

impl Panel {
    /// construct once after BSS initialization. The channel bank
    /// and settings baseline must have no other references or ISR users.
    #[inline(never)]
    pub(super) unsafe fn new() -> Self {
        let mut operation = Operation::new();
        let _ = operation.configure(Setting::Rx, 1);

        let mut panel = Self {
            screen: Screen::Home,
            last: None,
            light: true,
            period: 250,
            sounds: super::sound::Settings::new(),
            slope: crate::slope::Settings::new(),
            operation,
            dual: super::dual::Watch::new(),
            bank: unsafe { operation::channel_bank() },
            menu: Menu::new(),
            browser: Browser::new(),
            range_editor: None,
            route_editor: None,
            signal_editor: None,
            #[cfg(feature = "spectrum")]
            scope: unsafe { super::spectrum::Scope::new() },
            import_count: 0,
            import_decision: None,
            import_ready_at: 0,
            channel_editor: None,
            changes: unsafe { Changes::new() },
            preferences: super::preferences::Preferences::new(),
            appearance: super::appearance::Settings::new(),
            cue: None,
            autosave: super::preferences::Autosave::new(),
            automatic_save: false,
            unsaved_warning: false,
            file_action: None,
            ram_reset: false,
            f_held: false,
            ham: true,
            power: 0,
            tot: crate::app::tx_options::DEFAULT_TOT,
            warning_stage: 0,
            test_action: None,
            test_refused: false,
        };

        // Defaults are the baseline until a checked saved record is applied.
        panel.accept_baseline();
        panel
    }

    fn values(&self) -> Values {
        let tune = self.operation.tune(self.bank);
        let request = self.operation.request(self.bank);

        Values {
            rx: request.enabled,
            dual_watch: self.dual.enabled(),
            vfo: self.operation.active,
            channel: matches!(self.operation.mode, Mode::Channel),
            populated: tune.is_some(),
            slot_populated: self.bank.channel(self.operation.slot).is_some(),
            uhf: request.frequency >= 280_000_000,
            narrow: request.narrow,
            am: request.am,
            step: tune.map_or(12_500, |tune| tune.step),
            sql: self.operation.squelch,
            monitor: request.monitor,
            light: self.light,
            period: self.period,
            slot: self.operation.slot,
            sounds: self.sounds,
            slope: self.slope,
            slope_frequency: self.operation.transmit_with(self.bank, self.slope),
            ham: self.ham,
            power: self.power,
            tot: self.tot,
            tx_signal: self.operation.vfo_signal(),
            rx_signal: self.operation.vfo_rx_signal(),
            dirty: self.changes.dirty(),
            preferences: self.preferences,
            appearance: self.appearance,
        }
    }

    #[inline(never)]
    fn reconcile_settings(&mut self) {
        let current = self.saved_state();
        self.changes.compare(&current);
        self.autosave.changed(crate::mcu::tick::snapshot());
    }

    #[inline(never)]
    fn accept_baseline(&mut self) {
        let current = self.saved_state();
        self.changes.accept(&current);
        self.autosave.checked(crate::mcu::tick::snapshot());
        self.automatic_save = false;
    }

    /// Call only after the journal publishes successful committed completion.
    pub(super) fn saved(&mut self) {
        self.accept_baseline();
        self.ram_reset = false;
        self.cue = Some(super::feedback::Cue::Saved);
    }

    fn open_menu(&mut self) {
        self.operation.stop_scan();
        self.f_held = false;
        self.menu.open();
        self.screen = Screen::Menu;
    }

    fn open_channel_editor(&mut self, slot: usize) {
        if let Some(channel) = self.bank.channel(slot) {
            self.operation.disable();
            self.channel_editor = Some(super::channel::Editor::new(slot, channel));
            self.screen = Screen::Channel;
        }
    }

    fn apply_channel(&mut self, effect: ChannelEffect) {
        self.cue = Some(match effect {
            ChannelEffect::Home => super::feedback::Cue::Back,
            ChannelEffect::Delete(_) => super::feedback::Cue::Delete,
            _ => super::feedback::Cue::Apply,
        });

        match effect {
            ChannelEffect::Home => {
                self.screen = Screen::Home;
            }
            ChannelEffect::Use(slot) => {
                if self.bank.channel(slot).is_some() {
                    self.operation.stop_scan();
                    let _ = self.operation.configure(Setting::Slot, slot as u32);
                    let _ = self.operation.configure(Setting::Mode, 1);
                    self.screen = Screen::Home;
                }
            }
            ChannelEffect::Edit(slot) => {
                self.open_channel_editor(slot);
            }
            ChannelEffect::Copy {
                source,
                destination,
            } => {
                self.operation.disable();

                if self.bank.copy_slot(source, destination) {
                    self.browser.after_change(destination);
                }
            }
            ChannelEffect::Move {
                source,
                destination,
            } => {
                self.operation.disable();

                if self.bank.move_slot(source, destination) {
                    self.browser.after_change(destination);
                }
            }
            ChannelEffect::Delete(slot) => {
                self.operation.disable();

                if self.bank.delete(slot) {
                    self.browser.after_change(slot);
                }
            }
            ChannelEffect::Store(slot) => {
                if let Some(vfo) = self.operation.storable_vfo() {
                    self.operation.disable();
                    if self.bank.store_vfo(slot, vfo, self.operation.vfo_signal(), self.operation.vfo_rx_signal()) {
                        self.browser.after_change(slot);
                    }
                } else {
                    self.cue = Some(super::feedback::Cue::Reject);
                }
            }
        }
    }

    fn apply(&mut self, effect: Effect) {
        self.cue = Some(match &effect {
            Effect::Home => super::feedback::Cue::Back,
            Effect::Set(_, _) | Effect::File(_) => super::feedback::Cue::Apply,
            Effect::Action(Action::DeleteChannel) => super::feedback::Cue::Delete,
            _ => super::feedback::Cue::Enter,
        });

        match effect {
            Effect::Home => {
                if self.preferences.autosave == 1 && self.changes.dirty() {
                    self.unsaved_warning = true;
                } else {
                    self.screen = Screen::Home;
                }
            }
            Effect::Set(Setting::SlopeEnabled, value) => {
                if cfg!(feature = "fam") && value < 2 {
                    self.slope.enabled = value != 0;
                }
            }
            Effect::Set(Setting::SlopeSide, value) => {
                if cfg!(feature = "fam") && value < 2 {
                    self.slope.upper = value != 0;
                }
            }
            Effect::Set(Setting::SlopeDelta, value) => {
                if cfg!(feature = "fam") && value < 500 {
                    self.slope.delta = value as u16 + 1;
                }
            }
            Effect::Set(Setting::SlopeWidth, value) => {
                if cfg!(feature = "fam") && value < 2 {
                    self.slope.narrow = value != 0;
                }
            }
            Effect::Set(Setting::DualWatch, value) => {
                if value < 2 {
                    self.operation.stop_scan();
                    self.dual.set(value != 0);
                }
            }
            Effect::Set(Setting::SpectrumEnabled, value) => {
                self.preferences.spectrum = value != 0;
            }
            Effect::Set(Setting::SpectrumStep, value) => {
                self.preferences.spectrum_step = value as u8;
            }
            Effect::Set(Setting::SpectrumRssi, value) => {
                self.preferences.spectrum_rssi = if value == 32 {
                    511
                } else {
                    (value * 16) as u16
                };
            }
            Effect::Set(Setting::SpectrumNoise, value) => {
                self.preferences.spectrum_noise = if value == 8 {
                    127
                } else {
                    (value * 16) as u8
                };
            }
            Effect::Set(Setting::Autosave, value) => {
                self.preferences.autosave = value as u8;
            }
            Effect::Set(Setting::ScanDwell, value) => {
                self.preferences.dwell = value as u8;
                self.operation.scan_preferences(self.preferences);
            }
            Effect::Set(Setting::ScanHold, value) => {
                self.preferences.hold = value as u8;
                self.operation.scan_preferences(self.preferences);
            }
            Effect::Set(Setting::ScanResume, value) => {
                self.preferences.resume = value as u8;
                self.operation.scan_preferences(self.preferences);
            }
            Effect::Set(Setting::ScanDirection, value) => {
                self.preferences.reverse = value != 0;
                self.operation.scan_preferences(self.preferences);
            }
            Effect::Set(Setting::ScanSkip, value) => {
                self.preferences.set_skipped(self.operation.slot, value != 0);
                self.operation.scan_preferences(self.preferences);
            }
            Effect::Set(Setting::Ham, 0) => {
                self.operation.stop_scan();
                self.warning_stage = 1;
                self.screen = Screen::HamWarning;
            }
            Effect::Set(Setting::Ham, _) => {
                self.ham = true;
                self.warning_stage = 0;
            }
            Effect::Set(Setting::Power, value) => {
                if value < crate::app::tx_options::POWER_COUNT as u32 {
                    self.power = value as u8;
                }
            }
            Effect::Set(Setting::Tot, value) => {
                if value < crate::app::tx_options::timeout_count() {
                    self.tot = value as u8;
                }
            }
            Effect::Set(Setting::Roger, value) => {
                self.sounds.roger = value as u8;
            }
            Effect::Set(Setting::Permit, value) => {
                self.sounds.permit = value as u8;
            }
            Effect::Set(Setting::BootTone, value) => {
                self.sounds.boot = value != 0;
            }
            Effect::Set(Setting::Light, value) => {
                self.light = value != 0;
            }
            Effect::Set(Setting::Refresh, value) => {
                self.period = match value {
                    0 => 125,
                    1 => 250,
                    _ => 500,
                };
            }
            Effect::Set(setting, value) => {
                if !self.appearance.configure(setting, value) {
                    let _ = self.operation.configure(setting, value);
                }
            }
            Effect::Action(action) => match action {
                Action::RxSignal => {
                    if self.operation.tx_route().is_some() {
                        self.operation.stop_scan();
                        self.signal_editor = Some(super::tx_signal::Editor::new(
                            self.operation.vfo_rx_signal(),
                        ));
                        self.screen = Screen::RxSignal;
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                Action::TxSignal => {
                    if self.operation.tx_route().is_some() {
                        self.operation.stop_scan();
                        self.signal_editor = Some(super::tx_signal::Editor::new(
                            self.operation.vfo_signal(),
                        ));
                        self.screen = Screen::TxSignal;
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                Action::TxRoute => {
                    if let Some(route) = self.operation.tx_route() {
                        self.operation.stop_scan();
                        self.route_editor = Some(super::tx_route::Editor::new(
                            self.operation.active_vfo().frequency,
                            route,
                        ));
                        self.screen = Screen::TxRoute;
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                Action::ScanRange => {
                    self.operation.disable();
                    self.range_editor = Some(super::scan_range::Editor::new(
                        self.preferences.lower,
                        self.preferences.upper,
                    ));
                    self.screen = Screen::ScanRange;
                }
                Action::Spectrum => {
                    #[cfg(feature = "spectrum")]
                    {
                        self.operation.stop_scan();
                        self.scope.start(
                            crate::mcu::tick::snapshot(),
                            self.operation.request(self.bank),
                            self.preferences,
                        );
                        self.screen = Screen::Spectrum;
                    }

                    #[cfg(not(feature = "spectrum"))]
                    {
                        // The selected menu catalog has no Spectrum action.
                        // A direct internal request still cannot retune RF.
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                Action::Frequency => {
                    self.operation.handle(press(Key::Eight), self.bank);
                    self.screen = Screen::Home;
                }
                Action::Scan => {
                    self.operation.handle(press(Key::Nine), self.bank);
                    self.screen = Screen::Home;
                }
                Action::SaveChannel => {
                    self.operation.handle(press(Key::Five), self.bank);
                    self.screen = Screen::Home;
                }
                Action::ChannelList => {
                    self.operation.stop_scan();
                    self.browser.open(self.operation.slot);
                    self.screen = Screen::Channels;
                }
                Action::EditChannel => {
                    self.open_channel_editor(self.operation.slot);
                }
                Action::DeleteChannel => {
                    self.operation.disable();
                    let _ = self.bank.delete(self.operation.slot);
                    self.screen = Screen::Home;
                }
                Action::System => {
                    self.screen = Screen::System;
                }
                Action::Input => {
                    self.screen = Screen::Input;
                }
                Action::Memory => {
                    self.screen = Screen::Memory;
                }
                Action::Storage => {
                    self.ram_reset = false;
                    self.screen = Screen::Files;
                }
                Action::Help => {
                    self.screen = Screen::Help;
                }
                Action::Stacks => {
                    self.screen = Screen::Stacks;
                }
                Action::Power => {
                    self.screen = Screen::Power;
                }
                Action::Radio => {
                    self.screen = Screen::Radio;
                }
                Action::Runtime => {
                    self.screen = Screen::Runtime;
                }
                Action::Layout => {
                    self.screen = Screen::Layout;
                }
                Action::About => {
                    self.screen = Screen::About;
                }
                Action::Serial => {
                    if cfg!(feature = "uart") {
                        self.screen = Screen::Serial;
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                Action::LastFault => {
                    self.test_refused = false;
                    self.screen = Screen::LastFault;
                }
            },
            Effect::File(action) => {
                self.operation.disable();
                self.file_action = Some(action);
                self.ram_reset = false;
                self.screen = Screen::Files;
            }
            Effect::Test(test) => {
                self.operation.disable();
                self.test_action = Some(test);
                self.test_refused = false;
                self.screen = Screen::LastFault;
            }
        }
    }

    /// Persistent changes are reconciled after dispatch. Draft edits,
    /// cursor motion, and cancelled candidates leave the baseline unchanged.
    #[inline(never)]
    pub(super) fn handle(&mut self, event: Event) {
        self.last = Some(event);

        if matches!(event.key, Key::Ptt) {
            return;
        }

        if matches!(event.edge, Edge::Down) {
            self.cue = Some(match event.key {
                Key::Menu => super::feedback::Cue::Enter,
                Key::Exit => super::feedback::Cue::Back,
                _ => super::feedback::Cue::Key,
            });
        }

        self.dispatch(event);
        self.reconcile_settings();
    }

    fn dispatch(&mut self, event: Event) {
        if matches!(self.screen, Screen::TxSignal | Screen::RxSignal) {
            let receive = matches!(self.screen, Screen::RxSignal);
            let effect = self.signal_editor.as_mut().and_then(|editor| editor.key(event));
            match effect {
                Some(super::tx_signal::Effect::Cancel) => {
                    self.signal_editor = None;
                    self.screen = Screen::Menu;
                }
                Some(super::tx_signal::Effect::Apply(value)) => {
                    let applied = if receive {
                        self.operation.set_vfo_rx_signal(value)
                    } else {
                        self.operation.set_vfo_signal(value)
                    };
                    if applied {
                        self.signal_editor = None;
                        self.screen = Screen::Menu;
                        self.cue = Some(super::feedback::Cue::Apply);
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                None => {}
            }
            return;
        }

        if matches!(self.screen, Screen::TxRoute) {
            let effect = self.route_editor.as_mut().and_then(|editor| editor.key(event));
            match effect {
                Some(super::tx_route::Effect::Refuse) => {
                    self.cue = Some(super::feedback::Cue::Reject);
                }
                Some(super::tx_route::Effect::Cancel) => {
                    self.route_editor = None;
                    self.screen = Screen::Menu;
                }
                Some(super::tx_route::Effect::Apply(route)) => {
                    if self.operation.set_tx_route(route) {
                        self.route_editor = None;
                        self.screen = Screen::Menu;
                        self.cue = Some(super::feedback::Cue::Apply);
                    } else {
                        self.cue = Some(super::feedback::Cue::Reject);
                    }
                }
                None => {}
            }
            return;
        }

        if matches!(self.screen, Screen::Import) {
            if !crate::app::time::reached(
                crate::mcu::tick::snapshot(), self.import_ready_at,
            ) {
                return;
            }

            if matches!(event.edge, Edge::Down) {
                match event.key {
                    Key::Menu => self.import_decision = Some(true),
                    Key::Exit => self.import_decision = Some(false),
                    _ => {}
                }
            }
            return;
        }

        if matches!(self.screen, Screen::ScanRange) {
            let effect = self.range_editor.as_mut().and_then(|editor| editor.key(event));

            match effect {
                Some(super::scan_range::Effect::Refuse) => {
                    self.cue = Some(super::feedback::Cue::Reject);
                }
                Some(super::scan_range::Effect::Cancel) => {
                    self.range_editor = None;
                    self.screen = Screen::Menu;
                }
                Some(super::scan_range::Effect::Apply(lower, upper)) => {
                    self.preferences.lower = lower;
                    self.preferences.upper = upper;
                    self.operation.scan_preferences(self.preferences);
                    self.range_editor = None;
                    self.screen = Screen::Menu;
                }
                None => {}
            }
            return;
        }

        #[cfg(feature = "spectrum")]
        if matches!(self.screen, Screen::Spectrum) {
            let effect = self.scope.key(event, crate::mcu::tick::snapshot());

            match effect {
                Some(super::spectrum::Effect::Exit) => {
                    self.screen = Screen::Home;
                }
                Some(super::spectrum::Effect::Tune(frequency)) => {
                    let _ = self.operation.select_frequency(frequency);
                    self.screen = Screen::Home;
                }
                None => {}
            }
            return;
        }

        if self.unsaved_warning {
            if matches!(event.edge, Edge::Down) {
                match event.key {
                    Key::Menu => {
                        self.operation.disable();
                        self.file_action = Some(FileAction::Save);
                        self.automatic_save = false;
                        self.unsaved_warning = false;
                        self.screen = Screen::Files;
                    }
                    Key::Exit => {
                        self.unsaved_warning = false;
                        self.screen = Screen::Home;
                    }
                    _ => {}
                }
            }

            return;
        }

        if matches!(self.screen, Screen::HamWarning) {
            if matches!(event.edge, Edge::Down) {
                match event.key {
                    Key::Exit => {
                        self.warning_stage = 0;
                        self.screen = Screen::Menu;
                    }
                    Key::Menu if self.warning_stage == 1 => {
                        self.warning_stage = 2;
                    }
                    Key::Menu if self.warning_stage == 2 => {
                        self.ham = false;
                        self.warning_stage = 0;
                        self.screen = Screen::Menu;
                    }
                    _ => {}
                }
            }

            return;
        }

        if matches!(self.screen, Screen::Channels) {
            if let Some(effect) = self.browser.key(event, self.bank) {
                self.apply_channel(effect);
            }

            return;
        }

        if matches!(self.screen, Screen::Channel) {
            let effect = self.channel_editor
                .as_mut()
                .and_then(|editor| editor.key(event));

            match effect {
                Some(super::channel::Effect::Refuse) => {
                    self.cue = Some(super::feedback::Cue::Reject);
                }
                Some(super::channel::Effect::Cancel) => {
                    self.channel_editor = None;
                    self.browser.open(self.operation.slot);
                    self.screen = Screen::Channels;
                }
                Some(super::channel::Effect::Apply(slot, channel)) => {
                    if self.bank.update(slot, channel) {
                        self.cue = Some(super::feedback::Cue::Apply);
                        self.operation.disable();
                        let _ = self.operation.configure(Setting::Slot, slot as u32);
                        let _ = self.operation.configure(Setting::Mode, 1);

                        self.channel_editor = None;
                        self.browser.after_change(slot);
                        self.screen = Screen::Channels;
                    }
                }
                None => {}
            }

            return;
        }

        if matches!(self.screen, Screen::Home) && self.operation.editing() {
            self.operation.handle(event, self.bank);
            if matches!(event.edge, Edge::Down) && self.operation.entry_refused() {
                self.cue = Some(super::feedback::Cue::Reject);
            }
            return;
        }

        if matches!(event.key, Key::Side1) && matches!(event.edge, Edge::Down) {
            self.light = !self.light;
            return;
        }

        if matches!(self.screen, Screen::Menu) {
            let values = self.values();

            if let Some(effect) = self.menu.key(event, values) {
                self.apply(effect);
            }

            return;
        }

        if !matches!(self.screen, Screen::Home) {
            if matches!(event.edge, Edge::Down) {
                match event.key {
                    Key::Menu => self.open_menu(),
                    Key::Exit | Key::Star => self.screen = Screen::Home,
                    _ => {}
                }
            }

            return;
        }

        if matches!(event.key, Key::F) {
            match event.edge {
                Edge::Down => {
                    self.f_held = false;
                }
                Edge::Hold => {
                    self.f_held = true;
                    self.operation.handle(press(Key::Zero), self.bank);
                }
                Edge::Up if !self.f_held => {
                    self.operation.handle(press(Key::Six), self.bank);
                }
                _ => {}
            }

            return;
        }

        if matches!(event.edge, Edge::Down) {
            match event.key {
                Key::Menu => {
                    self.open_menu();
                    return;
                }
                Key::Exit => {
                    self.operation.disable();
                    return;
                }
                Key::Side2 => {
                    self.operation.handle(press(Key::Menu), self.bank);
                    return;
                }
                Key::Star => {
                    self.operation.handle(press(Key::Four), self.bank);
                    return;
                }
                key if digit(key) => {
                    self.operation.handle(press(Key::Eight), self.bank);
                    self.operation.handle(event, self.bank);
                    return;
                }
                _ => {}
            }
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.operation.handle(event, self.bank);
        }
    }

    pub(super) fn export_channels(&self, bytes: &mut [u8]) {
        self.bank.export_wire(bytes);
    }

    pub(super) fn prepare_import(&mut self, bytes: &[u8], version: u8, now: u32) -> bool {
        let count = match Bank::validate_wire(bytes, version) {
            Some(count) => count,
            None => return false,
        };

        self.operation.disable();
        #[cfg(feature = "spectrum")]
        self.scope.stop();
        self.range_editor = None;
        self.route_editor = None;
        self.signal_editor = None;
        self.channel_editor = None;
        self.unsaved_warning = false;
        self.import_count = count;
        self.import_decision = None;
        self.import_ready_at = now.wrapping_add(250);
        self.screen = Screen::Import;

        true
    }

    pub(super) fn take_import_decision(&mut self) -> Option<bool> {
        self.import_decision.take()
    }

    pub(super) fn finish_import(&mut self, bytes: &[u8], version: u8, accept: bool) {
        if accept && self.bank.import_wire(bytes, version) {
            self.cue = Some(super::feedback::Cue::Apply);
            self.operation.disable();
            self.preferences.skipped = 0;
            self.operation.scan_preferences(self.preferences);
            // Bank replacement does not change the independent HAM setting.
            self.browser.open(self.operation.slot);
            self.screen = Screen::Channels;
            self.reconcile_settings();
        } else {
            self.cue = Some(super::feedback::Cue::Reject);
            self.screen = Screen::Home;
        }
    }

    pub(super) fn take_test_action(&mut self) -> Option<crate::debug::Test> {
        self.test_action.take()
    }

    pub(super) fn refuse_test(&mut self) {
        self.test_refused = true;
        self.screen = Screen::LastFault;
    }

    /// Save uses the ordinary journal and its existing exact completion event.
    /// Automatic scheduling does not accept a baseline before commit succeeds.
    /// The caller excludes carrier activity, PTT, TX, boot cues, and busy I/O.
    pub(super) fn autosave_poll(
        &mut self,
        now: u32,
        eligible: bool,
        journal_busy: bool,
        journal_error: u32,
    ) {
        if self.automatic_save && !journal_busy && journal_error != 0 {
            self.autosave.failed();
            self.automatic_save = false;
        }

        let scope = matches!(self.screen, Screen::Home)
            && !self.operation.editing()
            && !self.operation.scanning()
            && !self.unsaved_warning
            && self.file_action.is_none()
            && !journal_busy
            && journal_error == 0;

        if self.autosave.due(
            now,
            self.preferences.autosave,
            self.changes.dirty(),
            eligible && scope,
        ) {
            self.autosave.attempted(now);
            self.automatic_save = true;
            self.file_action = Some(FileAction::Save);

            // Keep the requested RX state. The foreground temporarily masks
            // it while the journal is busy, then reception resumes normally.
        }
    }

    pub(super) fn take_file_action(&mut self) -> Option<FileAction> {
        self.file_action.take()
    }

    #[inline(never)]
    pub(super) fn saved_state(&self) -> SavedState {
        let mut state = self.operation.snapshot(self.bank, self.light, self.period);
        state.words[6] = self.sounds.word();
        state.words[7] = self.power as u32;
        self.preferences.write(&mut state.words);
        state.words[108] = self.appearance.word();
        state.words[109] = self.tot as u32;
        state.words[110] = (!self.ham) as u32;
        state.words[111] = self.dual.enabled() as u32
            | ((self.slope.word() as u32) << 1);
        state
    }

    pub(super) fn appearance(&self) -> super::appearance::Settings {
        self.appearance
    }

    pub(super) fn take_cue(&mut self) -> Option<super::feedback::Cue> {
        self.cue.take()
    }

    pub(super) fn feedback_scope(&self) -> bool {
        let eligible = !self.dual_active() && !self.operation.scanning()
            && !matches!(self.screen, Screen::Import | Screen::HamWarning | Screen::TxSignal | Screen::RxSignal);

        #[cfg(feature = "spectrum")]
        {
            eligible && !self.scope.active()
                && !matches!(self.screen, Screen::Spectrum)
        }

        #[cfg(not(feature = "spectrum"))]
        {
            eligible
        }
    }

    pub(super) fn sound_settings(&self) -> super::sound::Settings {
        self.sounds
    }

    #[inline(never)]
    pub(super) fn restore(&mut self, state: &SavedState) -> bool {
        let sounds = match super::sound::Settings::decode(state.words[6]) {
            Some(sounds) => sounds,
            None => return false,
        };

        let preferences = match super::preferences::Preferences::read(&state.words) {
            Some(preferences) => preferences,
            None => return false,
        };

        let appearance = match super::appearance::Settings::decode(state.words[108]) {
            Some(appearance) => appearance,
            None => return false,
        };

        let slope = match crate::slope::Settings::decode(state.words[111] >> 1) {
            Some(slope) => slope,
            None => return false,
        };

        if !self.operation.restore(self.bank, state) {
            return false;
        }

        self.appearance = appearance;
        self.cue = None;
        self.preferences = preferences;
        self.operation.scan_preferences(preferences);
        #[cfg(feature = "spectrum")]
        self.scope.stop();
        self.range_editor = None;
        self.import_decision = None;
        self.unsaved_warning = false;
        self.automatic_save = false;
        self.sounds = sounds;
        self.power = state.words[7] as u8;
        self.tot = state.words[109] as u8;
        self.route_editor = None;
        self.signal_editor = None;
        self.dual.set(state.words[111] & 1 != 0);
        self.slope = slope;
        self.ham = state.words[110] == 0;
        self.warning_stage = 0;
        let _ = self.operation.configure(Setting::Rx, 1);

        self.light = state.words[0] != 0;
        self.period = state.words[1];
        self.file_action = None;
        self.ram_reset = false;
        self.channel_editor = None;
        self.test_action = None;
        self.test_refused = false;
        self.f_held = false;

        self.menu.open();
        self.browser.open(self.operation.slot);
        self.accept_baseline();

        true
    }

    /// Clears the volatile bank without changing the saved-state baseline.
    pub(super) fn reset_ram(&mut self) {
        self.operation = Operation::new();
        let _ = self.operation.configure(Setting::Rx, 1);

        self.power = 0;
        self.tot = crate::app::tx_options::DEFAULT_TOT;
        self.route_editor = None;
        self.signal_editor = None;
        self.dual = super::dual::Watch::new();
        self.ham = true;
        self.warning_stage = 0;
        self.preferences = super::preferences::Preferences::new();
        self.appearance = super::appearance::Settings::new();
        self.cue = None;
        self.operation.scan_preferences(self.preferences);
        #[cfg(feature = "spectrum")]
        self.scope.stop();
        self.range_editor = None;
        self.import_decision = None;
        self.unsaved_warning = false;
        self.automatic_save = false;
        self.bank.clear();
        self.light = true;
        self.period = 250;
        self.sounds = super::sound::Settings::new();
        self.slope = crate::slope::Settings::new();
        self.file_action = None;
        self.ram_reset = true;
        self.channel_editor = None;
        self.test_action = None;
        self.test_refused = false;
        self.f_held = false;

        self.menu.open();
        self.browser.open(0);
        self.reconcile_settings();
    }

    pub(super) fn ham_mode(&self) -> bool {
        self.ham
    }

    pub(super) fn power(&self) -> u8 {
        self.power
    }

    pub(super) fn tot_ticks(&self) -> u32 {
        match crate::app::tx_options::timeout_ticks(self.tot) {
            Some(ticks) => ticks,
            None => crate::startup::stop(0x700A),
        }
    }

    pub(super) fn tx_scope(&self) -> bool {
        matches!(self.screen, Screen::Home)
            && !self.operation.editing()
            && !self.operation.scanning()
            && self.operation.request(self.bank).enabled
    }

    /// DW runs only on the normal VFO screen with carrier SQL available.
    /// Other modes preserve its session preference but use selected RX.
    pub(super) fn dual_active(&self) -> bool {
        let request = self.operation.request(self.bank);
        self.dual.enabled()
            && matches!(self.screen, Screen::Home)
            && matches!(self.operation.mode, Mode::Vfo)
            && !self.operation.editing()
            && !self.operation.scanning()
            && !self.scope_active()
            && request.enabled
            && !request.monitor
            && request.squelch != 0
    }

    /// Called before constructing the request used by the RF task.
    pub(super) fn prepare_receive(&mut self, pinned: bool) {
        let eligible = self.dual_active();
        self.dual.prepare(self.operation.active, eligible, pinned);
    }

    pub(super) fn receive_request(&self) -> Request {
        #[cfg(feature = "spectrum")]
        if self.scope.active() {
            return self.scope.request();
        }

        let mut request = if self.dual_active() {
            self.operation.vfo_request(self.dual.listening())
        } else {
            self.operation.request(self.bank)
        };
        if matches!(self.screen, Screen::TxRoute | Screen::TxSignal | Screen::RxSignal) {
            request.enabled = false;
        }
        request
    }

    pub(super) fn slope_settings(&self) -> crate::slope::Settings {
        self.slope
    }

    pub(super) fn tx_signal(&self) -> u16 {
        if cfg!(feature = "fam") && self.slope.enabled
            && self.operation.request(self.bank).am
        {
            0
        } else {
            self.operation.tx_signal(self.bank)
        }
    }

    pub(super) fn transmit_frequency(&self) -> Result<u32, u32> {
        self.operation.transmit_with(self.bank, self.slope)
    }

    pub(super) fn scope_active(&self) -> bool {
        #[cfg(feature = "spectrum")]
        {
            self.scope.active()
        }

        #[cfg(not(feature = "spectrum"))]
        {
            false
        }
    }

    pub(super) fn observe(&mut self, now: u32, receiver: View, ptt: bool) {
        #[cfg(feature = "spectrum")]
        if self.scope.active() {
            self.scope.observe(now, receiver);
            return;
        }

        if self.dual_active() && !ptt {
            let request = self.operation.vfo_request(self.dual.listening());
            let settled = matches!(
                receiver.phase, crate::drivers::bk4829::receive::Phase::Receive
            ) && receiver.frequency == request.frequency
                && receiver.narrow == request.narrow
                && receiver.am == request.am
                && receiver.rx_signal == request.rx_signal;
            self.dual.observe(now, settled, receiver.signal);
        }

        let previous_frequency = self.operation.request(self.bank).frequency;
        let previous_slot = self.operation.slot;

        self.operation.observe(now, receiver, ptt, self.bank);

        let current_frequency = self.operation.request(self.bank).frequency;

        if current_frequency != previous_frequency || self.operation.slot != previous_slot {
            self.reconcile_settings();
        }

        self.operation.publish(now, self.bank);
    }

    #[inline(never)]
    pub(super) fn render(
        &self,
        frame: &mut Frame,
        state: Snapshot,
        input: &Input,
        battery: Reading,
        memory: crate::storage::View,
        receiver: View,
        journal: JournalView,
        transmitter: TxView,
    ) {
        if transmitter.active {
            screens::transmit(frame, transmitter, battery, memory, self.appearance);
            return;
        }

        if self.unsaved_warning {
            frame.clear();
            frame.text(2, 0, b"UNSAVED CHANGES");
            frame.fill(0, 8, 128, 1, true);
            frame.text_size(2, 18, b"SETTINGS DIFFER FROM SAVED DATA", ui::font::Size::Small);
            frame.text_size(2, 30, b"MENU SAVES THROUGH THE JOURNAL", ui::font::Size::Small);
            frame.text_size(2, 42, b"EXIT KEEPS CHANGES IN RAM", ui::font::Size::Small);
            frame.text_size(2, 58, b"MENU SAVE / EXIT KEEP", ui::font::Size::Small);
            return;
        }

        match self.screen {
            Screen::TxSignal | Screen::RxSignal => {
                if let Some(editor) = self.signal_editor.as_ref() {
                    editor.render(frame, matches!(self.screen, Screen::RxSignal));
                } else {
                    frame.clear();
                }
            }
            Screen::TxRoute => {
                if let Some(editor) = self.route_editor.as_ref() {
                    editor.render(frame);
                } else {
                    frame.clear();
                }
            }
            Screen::ScanRange => {
                if let Some(editor) = self.range_editor.as_ref() {
                    editor.render(frame);
                } else {
                    frame.clear();
                }
            }
            #[cfg(feature = "spectrum")]
            Screen::Spectrum => {
                self.scope.render(frame);
            }
            Screen::Import => {
                frame.clear();
                frame.text(2, 0, b"IMPORT CHANNEL BANK");
                frame.fill(0, 8, 128, 1, true);
                frame.text_size(2, 13, b"INCOMING", ui::font::Size::Small);
                ui::widgets::number(
                    frame, 46, 13, self.import_count, ui::font::Size::Small,
                );
                frame.text_size(
                    2, 25, b"ALL SIXTEEN RAM SLOTS REPLACED", ui::font::Size::Small,
                );
                frame.text_size(
                    2, 36, b"ABSENT FILE SLOTS BECOME EMPTY", ui::font::Size::Small,
                );
                frame.text_size(
                    2, 47, b"AUTOSAVE POLICY STILL APPLIES", ui::font::Size::Small,
                );
                frame.text_size(
                    2, 58, b"MENU APPLY / EXIT CANCEL", ui::font::Size::Small,
                );
            }
            Screen::Home => {
                if self.dual.enabled()
                    && matches!(self.operation.mode, Mode::Vfo)
                    && !self.operation.editing()
                    && !self.operation.scanning()
                {
                    screens::dual_home(
                        frame, &self.dual, &self.operation,
                        receiver, transmitter, battery, memory,
                        self.changes.dirty(),
                        self.slope,
                    );
                } else {
                    screens::home(
                        frame,
                        &self.operation,
                        self.bank,
                        input,
                        receiver,
                        transmitter,
                        battery,
                        memory,
                        self.changes.dirty(),
                        self.appearance,
                        self.slope,
                    );
                }
            }
            Screen::Channels => {
                self.browser.render(
                    frame,
                    self.bank,
                    self.operation.active_profile(),
                    self.changes.dirty(),
                    self.operation.storable_vfo().is_some(),
                );
            }
            Screen::Channel => {
                if let Some(editor) = self.channel_editor.as_ref() {
                    editor.render(frame);
                } else {
                    frame.clear();
                    frame.text(2, 18, b"NO CHANNEL DRAFT");
                }
            }
            Screen::Menu => {
                self.menu.render(frame, self.values());
            }
            Screen::HamWarning => {
                screens::ham_warning(frame, self.warning_stage);
            }
            Screen::Input => {
                if cfg!(feature = "debug-tools") {
                    screens::inputs(frame, state, input, battery, self.last);
                }
            }
            Screen::System => {
                if cfg!(feature = "debug-tools") {
                    ui::diagnostic(frame, state);
                }
            }
            Screen::Memory => {
                if cfg!(feature = "debug-tools") {
                    screens::memory(frame, memory);
                }
            }
            Screen::Files => {
                screens::files(frame, journal, self.ram_reset, self.changes.dirty());
            }
            Screen::Help => {
                screens::help(frame);
            }
            Screen::Stacks => {
                if cfg!(feature = "diagnostics") {
                    screens::stacks(frame, crate::debug::stack_view());
                }
            }
            Screen::LastFault => {
                if cfg!(feature = "diagnostics") {
                    screens::last_fault(frame, crate::debug::last_fault(), self.test_refused);
                }
            }
            Screen::Power => {
                if cfg!(feature = "diagnostics") {
                    screens::power(frame, battery, memory);
                }
            }
            Screen::Radio => {
                if cfg!(feature = "debug-tools") {
                    screens::radio(frame, receiver, self.operation.squelch);
                }
            }
            Screen::Runtime => {
                if cfg!(feature = "debug-tools") {
                    screens::runtime(frame, crate::debug::timing::view());
                }
            }
            Screen::Layout => {
                if cfg!(feature = "debug-tools") {
                    screens::layout(frame, crate::debug::memory_view(), self.bank);
                }
            }
            Screen::About => {
                screens::about(frame);
            }
            Screen::Serial => {
                if cfg!(feature = "diagnostics") {
                    screens::serial(frame, crate::drivers::uart::diagnostic());
                }
            }
        }
    }
}