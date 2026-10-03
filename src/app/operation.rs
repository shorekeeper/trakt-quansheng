//! Saved VFO/channel profiles, direct tuning, and carrier-resume scanning.
//!
//! A and B are independent VFOs, not simultaneous receivers. Each retains
//! an other-band frequency, TX route, and separate RX/TX signal selection.
//! Channel mode recalls a complete profile without modifying either VFO.
//! An empty slot disables reception and provides no transmit destination.
//!
//! The static bank contains sixteen explicit twenty-byte slots, totaling
//! 320 bytes. Each populated slot packs its profile and eight printable
//! ASCII name bytes. Empty slots are entirely zero. Copy and exchange retain
//! the complete profile; Store VFO replaces profile data and preserves an
//! occupied slot's name. Mutation never writes Flash directly.
//!
//! Scan waits for the requested receive profile, then uses configured dwell,
//! hold, direction, skipped-slot, and resume preferences. Carrier detection
//! is separate from signaling audio. Monitor, SQL zero, PTT, an RX failure,
//! direct input, and manual tuning stop scanning.
//!
//! Frequency entry uses six or seven integer kHz digits. An entry opened
//! at 1000 MHz or above selects seven digits; otherwise it selects six.
//! F on an empty field toggles the length. F on a nonempty field deletes
//! one digit. The final required digit automatically accepts an admitted
//! frequency. Prefixes are never guessed to mean a different input length.
//!
//! Invalid complete input stays visible without changing tuning. The next
//! digit starts a fresh candidate of the same length. EXIT discards entry;
//! MENU on incomplete input reports refusal rather than accepting a prefix.
//!
//! Reception is suppressed during entry. Automatic acceptance changes only
//! the selected VFO frequency, not its RX enable choice or TX route. Retained
//! offsets that no longer resolve must block TX rather than being clamped.
//!
//! Snapshot maps all persistent fields, including channel routes and signal
//! pairs. Restore validates the complete Snapshot before changing the bank
//! or operation state. Temporary RX admission starts disabled; Panel applies
//! its Load policy after successful restoration.

use core::ptr::addr_of_mut;

use crate::app::time::reached;
use crate::storage::record::Snapshot;
use crate::drivers::{
    bk4829::receive::{Band, Phase, Request, View},
    keyboard::{Edge, Event, Key},
};

pub(super) use crate::profile::Tune;

pub(super) use crate::channel::Channel;

#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    bytes: [u8; 20],
}

impl Slot {
    const EMPTY: Self = Self { bytes: [0; 20] };

    const fn empty(self) -> bool {
        let mut index = 0;
        while index < self.bytes.len() {
            if self.bytes[index] != 0 { return false; }
            index += 1;
        }
        true
    }

    const fn channel(self) -> Option<Channel> {
        match Channel::decode(&self.bytes, 9) {
            Some(channel) => channel,
            None => None,
        }
    }

    const fn tune(self) -> Option<Tune> {
        match self.channel() {
            Some(channel) => Some(channel.tune),
            None => None,
        }
    }
}

#[repr(C)]
pub(super) struct Bank {
    slots: [Slot; 16],
}

impl Bank {
    pub(super) const fn new() -> Self {
        Self { slots: [Slot::EMPTY; 16] }
    }

    pub(super) const fn clear(&mut self) {
        *self = Self::new();
    }

    pub(super) const fn get(&self, index: usize) -> Option<Tune> {
        if index >= 16 { None } else { self.slots[index].tune() }
    }

    pub(super) const fn channel(&self, index: usize) -> Option<Channel> {
        if index >= Self::capacity() {
            None
        } else {
            self.slots[index].channel()
        }
    }

    pub(super) const fn update(&mut self, index: usize, channel: Channel) -> bool {
        if index >= Self::capacity() { return false; }
        let mut slot = Slot::EMPTY;
        if !channel.encode(&mut slot.bytes) { return false; }
        self.slots[index] = slot;
        true
    }

    /// Copies the complete checked profile, including its name.
    /// Destination replacement requires confirmation by the UI owner.
    /// A failed validation leaves both slots unchanged.
    pub(super) const fn copy_slot(&mut self, source: usize, destination: usize) -> bool {
        if source >= Self::capacity()
            || destination >= Self::capacity()
            || source == destination
        {
            return false;
        }

        let channel = match self.channel(source) {
            Some(channel) => channel,
            None => return false,
        };

        self.update(destination, channel)
    }

    /// Moves into an empty destination or exchanges two populated slots.
    /// Exchanging preserves both profiles instead of silently discarding
    /// the destination. Both entries are validated before the first write.
    pub(super) const fn move_slot(&mut self, source: usize, destination: usize) -> bool {
        if source >= Self::capacity()
            || destination >= Self::capacity()
            || source == destination
            || self.channel(source).is_none()
        {
            return false;
        }

        if !self.slots[destination].empty()
            && self.channel(destination).is_none()
        {
            return false;
        }

        let original = self.slots[source];
        let displaced = self.slots[destination];

        self.slots[destination] = original;
        self.slots[source] = displaced;

        true
    }

    /// Store the complete active VFO profile, including route and directional
    /// signaling. An occupied slot retains its name; an empty slot receives spaces.
    /// The destination index and complete candidate validate before mutation.
    pub(super) const fn store_vfo(
        &mut self,
        destination: usize,
        profile: crate::profile::Profile,
        tx_signal: u16,
        rx_signal: u16,
    ) -> bool {
        let name = match self.channel(destination) {
            Some(channel) => channel.name,
            None => [b' '; 8],
        };
        self.update(destination, Channel {
            tune: profile.tune,
            transmit: profile.transmit,
            tx_signal,
            rx_signal,
            name,
        })
    }

    pub(super) const fn delete(&mut self, index: usize) -> bool {
        if index >= 16 || self.get(index).is_none() {
            return false;
        }
        self.slots[index] = Slot::EMPTY;
        true
    }

    const fn put(&mut self, index: usize, tune: Tune) -> bool {
        self.store_vfo(index, crate::profile::Profile::simplex(tune), 0, 0)
    }

    pub(super) const fn capacity() -> usize {
        16
    }

    pub(super) const fn slot_bytes() -> usize {
        core::mem::size_of::<Slot>()
    }

    pub(super) const fn count(&self) -> u32 {
        let mut count = 0;
        let mut index = 0;
        while index < 16 {
            if self.get(index).is_some() { count += 1; }
            index += 1;
        }
        count
    }

    /// All wire schemas contain sixteen twenty-byte slots. Schema one
    /// has simplex fields, two adds routes, three adds TX CTCSS, and four
        /// adds independent RX CTCSS. Five adds tagged TX DCS; six adds RX DCS.
    /// Seven adds AM and aviation RX; eight extends RX to 50..<630 MHz.
    /// Nine adds 840..1300 MHz using the extra frequency bit.
    /// Older schemas retain their original frequency and field restrictions.
    /// All slots validate before live mutation.
    /// Reject the entire bank before any live slot is changed.
    pub(super) const fn validate_wire(bytes: &[u8], version: u8) -> Option<u32> {
        if bytes.len() != 320 || !matches!(version, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9) { return None; }
        let mut count = 0;
        let mut index = 0;
        while index < 16 {
            let slot = bytes.split_at(index * 20).1.split_at(20).0;
            match Channel::decode(slot, version) {
                Some(Some(_)) => count += 1,
                Some(None) => {}
                None => return None,
            }
            index += 1;
        }
        Some(count)
    }

    /// RAM slots use the same explicit schema-nine bytes as current export.
    /// The transport copies this bank into its stable output packet.
    pub(super) const fn export_wire(&self, bytes: &mut [u8]) {
        assert!(bytes.len() == 320);
        let mut index = 0;
        while index < 16 {
            let mut byte = 0;
            while byte < 20 {
                bytes[index * 20 + byte] = self.slots[index].bytes[byte];
                byte += 1;
            }
            index += 1;
        }
    }

    pub(super) const fn import_wire(&mut self, bytes: &[u8], version: u8) -> bool {
        if Self::validate_wire(bytes, version).is_none() { return false; }
        let mut index = 0;
        while index < 16 {
            let slot = bytes.split_at(index * 20).1.split_at(20).0;
            match Channel::decode(slot, version) {
                Some(Some(channel)) => assert!(self.update(index, channel)),
                Some(None) => self.slots[index] = Slot::EMPTY,
                None => panic!("validated channel changed during import"),
            }
            index += 1;
        }
        true
    }

    pub(super) const fn scan_count(&self, skipped: u16) -> u32 {
        let mut count = 0;
        let mut index = 0;

        while index < Self::capacity() {
            if skipped & (1u16 << index) == 0 && self.get(index).is_some() {
                count += 1;
            }

            index += 1;
        }

        count
    }

    const fn next_scan(
        &self,
        current: usize,
        skipped: u16,
        reverse: bool,
    ) -> Option<usize> {
        let mut distance = 1;

        while distance <= Self::capacity() {
            let index = if reverse {
                (current + Self::capacity() - distance) & 15
            } else {
                (current + distance) & 15
            };

            if skipped & (1u16 << index) == 0 && self.get(index).is_some() {
                return Some(index);
            }

            distance += 1;
        }

        None
    }

    const fn next(&self, current: usize) -> Option<usize> {
        let mut distance = 1;
        while distance <= 16 {
            let index = (current + distance) & 15;
            if self.get(index).is_some() { return Some(index); }
            distance += 1;
        }
        None
    }
}

#[used]
#[no_mangle]
static mut TRAKT_CHANNELS: Bank = Bank::new();

/// borrow once after BSS initialization. No other execution context
/// may access TRAKT_CHANNELS while the returned reference exists.
pub(super) unsafe fn channel_bank() -> &'static mut Bank {
    unsafe { &mut *addr_of_mut!(TRAKT_CHANNELS) }
}

#[derive(Clone, Copy)]
struct Vfo {
    tune: Tune,
    other: u32,
    transmit: crate::profile::Tx,
    tx_signal: u16,
    rx_signal: u16,
}

impl Vfo {
    const fn new(band: Band, other: Band) -> Self {
        Self {
            tune: Tune::initial(band),
            other: other.initial(),
            transmit: crate::profile::Tx::Simplex,
            tx_signal: 0,
            rx_signal: 0,
        }
    }

    const fn band(&self) -> Band {
        match Band::receive(self.tune.frequency) {
            Some(band) => band,
            None => panic!("invalid VFO"),
        }
    }

    const fn set_frequency(&mut self, frequency: u32) {
        let old_vhf = matches!(self.band(), Band::Vhf);
        let new_vhf = matches!(Band::receive(frequency), Some(Band::Vhf));
        if old_vhf != new_vhf {
            self.other = self.tune.frequency;
        }
        self.tune.frequency = frequency;
    }

    const fn switch_band(&mut self) {
        let old = self.tune.frequency;
        self.tune.frequency = self.other;
        self.other = old;
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub(super) enum Mode {
    Vfo,
    Channel,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum ScanPhase {
    Off,
    Wait,
    Dwell,
    Hold,
}

struct Scan {
    phase: ScanPhase,
    deadline: u32,
    hops: u32,
    preferences: crate::app::preferences::Preferences,
}

impl Scan {
    const fn new() -> Self {
        Self {
            phase: ScanPhase::Off,
            deadline: 0,
            hops: 0,
            preferences: crate::app::preferences::Preferences::new(),
        }
    }

    const fn active(&self) -> bool {
        !matches!(self.phase, ScanPhase::Off)
    }

    const fn stop(&mut self) {
        self.phase = ScanPhase::Off;
    }

    const fn poll(&mut self, now: u32, ready: bool, signal: bool) -> bool {
        if !self.active() { return false; }
        if !ready {
            self.phase = ScanPhase::Wait;
            return false;
        }
        if matches!(self.phase, ScanPhase::Wait) {
            self.phase = ScanPhase::Dwell;
            self.deadline = now.wrapping_add(self.preferences.dwell_ticks());
        }

        if signal {
            match self.preferences.resume {
                0 => {
                    self.phase = ScanPhase::Hold;
                    self.deadline = now.wrapping_add(self.preferences.hold_ticks());
                    return false;
                }
                1 => {
                    // Timed resume preserves the original dwell deadline.
                    self.phase = ScanPhase::Hold;
                }
                2 => {
                    self.stop();
                    return false;
                }
                _ => {
                    self.stop();
                    return false;
                }
            }
        }

        if reached(now, self.deadline) {
            self.phase = ScanPhase::Wait;
            self.hops = self.hops.saturating_add(1);
            return true;
        }
        false
    }

    const fn name(&self) -> &'static [u8] {
        match self.phase {
            ScanPhase::Off => b"STOPPED",
            ScanPhase::Wait => b"SCAN WAIT",
            ScanPhase::Dwell => b"SCANNING",
            ScanPhase::Hold => b"SCAN HOLD",
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    value: u32,
    digits: usize,
    wide: bool,
}

impl Entry {
    const fn new() -> Self {
        Self { value: 0, digits: 0, wide: false }
    }

    const fn push(&mut self, digit: u8) {
        if self.digits < crate::frequency_entry::count(self.wide) && digit < 10 {
            self.value = self.value * 10 + digit as u32;
            self.digits += 1;
        }
    }

    const fn frequency(&self) -> Option<u32> {
        match crate::frequency_entry::hertz(self.value, self.digits, self.wide) {
            Some(frequency) if Band::receive(frequency).is_some() => Some(frequency),
            _ => None,
        }
    }

    const fn preview(&self) -> [u8; 8] {
        crate::frequency_entry::preview(self.value, self.digits, self.wide)
    }
}

const fn digit(key: Key) -> Option<u8> {
    match key {
        Key::Zero => Some(0), Key::One => Some(1), Key::Two => Some(2),
        Key::Three => Some(3), Key::Four => Some(4), Key::Five => Some(5),
        Key::Six => Some(6), Key::Seven => Some(7), Key::Eight => Some(8),
        Key::Nine => Some(9), _ => None,
    }
}

#[repr(C)]
struct ControlState {
    signature: u32,
    tick: u32,
    enabled: u32,
    mode: u32,
    vfo: u32,
    slot: u32,
    channels: u32,
    scan: u32,
    hops: u32,
    frequency: u32,
    other_frequency: u32,
    step: u32,
    squelch: u32,
    narrow: u32,
    monitor: u32,
    editing: u32,
}

#[cfg(feature = "debug-tools")]
#[used]
#[no_mangle]
static mut TRAKT_CONTROL: ControlState = unsafe { core::mem::zeroed() };

pub(super) struct Operation {
    vfos: [Vfo; 2],
    pub(super) active: usize,
    pub(super) slot: usize,
    pub(super) mode: Mode,
    pub(super) squelch: u8,
    enabled: bool,
    monitor: bool,
    scan: Scan,
    entry: Option<Entry>,
    notice: u32,
}

impl Operation {
    pub(super) const fn new() -> Self {
        Self {
            vfos: [
                Vfo::new(Band::Vhf, Band::Uhf),
                Vfo::new(Band::Uhf, Band::Vhf),
            ],
            active: 0,
            slot: 0,
            mode: Mode::Vfo,
            squelch: 3,
            enabled: false,
            monitor: false,
            scan: Scan::new(),
            entry: None,
            notice: 0,
        }
    }

    #[cfg(feature = "spectrum")]
    pub(super) const fn select_frequency(&mut self, frequency: u32) -> bool {
        if Band::receive(frequency).is_none() {
            return false;
        }

        self.scan.stop();
        self.entry = None;
        self.mode = Mode::Vfo;
        self.vfos[self.active].set_frequency(frequency);
        self.enabled = true;
        self.notice = 0;
        true
    }

    /// Resolve transmission from the selected VFO or recalled channel profile.
    /// An empty channel has no destination. Route errors remain explicit and
    /// cannot authorize fallback to the receive frequency.
    pub(super) const fn transmit_frequency(&self, bank: &Bank) -> Result<u32, u32> {
        self.transmit_with(bank, crate::slope::Settings::new())
    }

    /// Resolve the selected profile without mutating its stored tune or route.
    /// SlopeCast applies only to AM and is refused when absent or disabled.
    pub(super) const fn transmit_with(
        &self, bank: &Bank, slope: crate::slope::Settings,
    ) -> Result<u32, u32> {
        let tune = match self.tune(bank) {
            Some(tune) => tune,
            None => return Err(0x7001),
        };

        let route = match self.mode {
            Mode::Vfo => self.vfos[self.active].transmit,
            Mode::Channel => match bank.channel(self.slot) {
                Some(channel) => channel.transmit,
                None => return Err(0x7001),
            },
        };

        crate::slope::resolve(crate::profile::Profile { tune, transmit: route }, slope)
    }

    pub(super) const fn tx_route(&self) -> Option<crate::profile::Tx> {
        if matches!(self.mode, Mode::Vfo) {
            Some(self.vfos[self.active].transmit)
        } else {
            None
        }
    }

    /// Apply a detached VFO edit only if its current RX/TX relationship
    /// resolves. Later RX tuning may block it, but must not erase it.
    pub(super) const fn set_tx_route(&mut self, route: crate::profile::Tx) -> bool {
        if !matches!(self.mode, Mode::Vfo)
            || route.frequency(self.vfos[self.active].tune.frequency).is_err()
        {
            return false;
        }
        self.vfos[self.active].transmit = route;
        self.scan.stop();
        self.notice = 0;
        true
    }

    pub(super) const fn active_profile(&self) -> crate::profile::Profile {
        crate::profile::Profile {
            tune: self.vfos[self.active].tune,
            transmit: self.vfos[self.active].transmit,
        }
    }

    pub(super) const fn storable_vfo(&self) -> Option<crate::profile::Profile> {
        let profile = self.active_profile();
        if profile.encode().is_ok() { Some(profile) } else { None }
    }

    pub(super) const fn displayed_route(&self, bank: &Bank) -> Option<crate::profile::Tx> {
        match self.mode {
            Mode::Vfo => self.tx_route(),
            Mode::Channel => match bank.channel(self.slot) {
                Some(channel) => Some(channel.transmit),
                None => None,
            },
        }
    }

    pub(super) const fn set_vfo_rx_signal(&mut self, value: u16) -> bool {
        if !matches!(self.mode, Mode::Vfo)
            || !crate::tx_signal::valid(value as u32)
        {
            return false;
        }
        self.vfos[self.active].rx_signal = value;
        self.scan.stop();
        self.notice = 0;
        true
    }

    pub(super) const fn vfo_rx_signal(&self) -> u16 {
        self.vfos[self.active].rx_signal
    }

    /// AM bypasses saved tone selectors without erasing either direction.
    pub(super) const fn rx_signal(&self, bank: &Bank) -> u16 {
        if matches!(self.tune(bank), Some(tune) if tune.am) {
            return 0;
        }
        match self.mode {
            Mode::Vfo => self.vfo_rx_signal(),
            Mode::Channel => match bank.channel(self.slot) {
                Some(channel) => channel.rx_signal,
                None => 0,
            },
        }
    }

    pub(super) const fn set_vfo_signal(&mut self, value: u16) -> bool {
        if !matches!(self.mode, Mode::Vfo)
            || !crate::tx_signal::valid(value as u32)
        {
            return false;
        }
        self.vfos[self.active].tx_signal = value;
        self.scan.stop();
        self.notice = 0;
        true
    }

    pub(super) const fn vfo_signal(&self) -> u16 {
        self.vfos[self.active].tx_signal
    }

    pub(super) const fn tx_signal(&self, bank: &Bank) -> u16 {
        match self.mode {
            Mode::Vfo => self.vfo_signal(),
            Mode::Channel => match bank.channel(self.slot) {
                Some(channel) => channel.tx_signal,
                None => 0,
            },
        }
    }

    pub(super) const fn vfo_profile(&self, index: usize) -> crate::profile::Profile {
        assert!(index < 2);
        crate::profile::Profile {
            tune: self.vfos[index].tune,
            transmit: self.vfos[index].transmit,
        }
    }

    pub(super) const fn vfo_request(&self, index: usize) -> Request {
        assert!(index < 2);
        let vfo = &self.vfos[index];
        Request {
            enabled: self.enabled && !self.editing(),
            frequency: vfo.tune.frequency,
            am: vfo.tune.am, narrow: vfo.tune.narrow,
            monitor: self.monitor,
            squelch: self.squelch,
            rx_signal: if vfo.tune.am { 0 } else { vfo.rx_signal },
        }
    }

    pub(super) const fn other_vfo(&self) -> Tune {
        self.vfos[self.active ^ 1].tune
    }

    pub(super) const fn active_vfo(&self) -> Tune {
        self.vfos[self.active].tune
    }

    pub(super) const fn tune(&self, bank: &Bank) -> Option<Tune> {
        match self.mode {
            Mode::Vfo => Some(self.vfos[self.active].tune),
            Mode::Channel => bank.get(self.slot),
        }
    }

    pub(super) const fn editing(&self) -> bool {
        self.entry.is_some()
    }

    /// Presentation only. Invalid entry remains detached from the live VFO.
    pub(super) const fn entry_refused(&self) -> bool {
        self.entry.is_some() && matches!(self.notice, 6 | 8)
    }

    pub(super) const fn preview(&self) -> Option<[u8; 8]> {
        match self.entry {
            Some(entry) => Some(entry.preview()),
            None => None,
        }
    }

    pub(super) const fn scan_preferences(
        &mut self,
        preferences: crate::app::preferences::Preferences,
    ) {
        assert!(preferences.valid());

        self.scan.preferences = preferences;
        self.scan.stop();
    }

    pub(super) const fn stop_scan(&mut self) {
        self.scan.stop();
    }

    pub(super) const fn disable(&mut self) {
        self.enabled = false;
        self.scan.stop();
        self.entry = None;
    }

    pub(super) const fn cycle_sql(&mut self) {
        self.squelch = if self.squelch == 9 { 0 } else { self.squelch + 1 };
        self.scan.stop();
    }

    pub(super) const fn scanning(&self) -> bool {
        self.scan.active()
    }

    pub(super) const fn scan_name(&self) -> &'static [u8] {
        self.scan.name()
    }

    pub(super) const fn notice(&self) -> &'static [u8] {
        match self.notice {
            1 => b"SAVED IN RAM",
            2 => b"SLOT SELECTED",
            3 => b"EMPTY CHANNEL",
            4 => b"SCAN NEEDS 2 CHANNELS",
            5 => b"SCAN: SQL ON, MON OFF",
            6 => b"REFUSE: RX OUT OF RANGE",
            7 => b"INVALID VFO PROFILE",
            8 => crate::frequency_entry::required(
                matches!(self.entry, Some(entry) if entry.wide),
            ),
            _ => b"",
        }
    }

    /// Applies one menu value. VFO-only fields cannot modify a recalled
    /// channel, and changing any receive option cancels scanning.
    /// Display settings are owned by Panel and are rejected here.
    pub(super) const fn configure(&mut self, setting: super::menu::Setting, value: u32) -> bool {
        use super::menu::Setting;

        match setting {


            Setting::Rx if value < 2 => self.enabled = value != 0,
            Setting::Vfo if value < 2 => {
                self.active = value as usize;
                self.mode = Mode::Vfo;
            }
            Setting::Mode if value < 2 => {
                self.mode = if value == 0 { Mode::Vfo } else { Mode::Channel };
            }
            Setting::Band if value < 2 && matches!(self.mode, Mode::Vfo) => {
                let vfo = &mut self.vfos[self.active];
                let uhf = matches!(vfo.band(), Band::Uhf);
                if uhf != (value != 0) { vfo.switch_band(); }
            }
            Setting::Demod if value < 2 && matches!(self.mode, Mode::Vfo) => {
                self.vfos[self.active].tune.am = value != 0;
            }
            Setting::Width if value < 2 && matches!(self.mode, Mode::Vfo) => {
                self.vfos[self.active].tune.narrow = value != 0;
            }
            Setting::Step if value < 3 && matches!(self.mode, Mode::Vfo) => {
                self.vfos[self.active].tune.step = match value {
                    0 => 12_500, 1 => 25_000, _ => 100_000,
                };
            }
            Setting::Sql if value < 10 => self.squelch = value as u8,
            Setting::Monitor if value < 2 => self.monitor = value != 0,
            Setting::Slot if value < 16 => self.slot = value as usize,
            _ => return false,
        }

        self.scan.stop();
        self.notice = 0;
        true
    }

    pub(super) fn request(&self, bank: &Bank) -> Request {
        let tune = self.tune(bank);
        let selected = tune.unwrap_or(self.vfos[self.active].tune);
        Request {
            enabled: self.enabled && tune.is_some() && !self.editing(),
            frequency: selected.frequency,
            am: selected.am, narrow: selected.narrow,
            monitor: self.monitor,
            squelch: self.squelch,
            rx_signal: self.rx_signal(bank),
        }
    }

    pub(super) const fn handle(&mut self, event: Event, bank: &mut Bank) {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return;
        }
        if matches!(event.key, Key::Ptt) {
            return;
        }
        self.notice = 0;

        if let Some(mut entry) = self.entry {
            if !matches!(event.edge, Edge::Down) { return; }

            if let Some(value) = digit(event.key) {
                // A complete valid candidate already closed the editor.
                // A retained complete value therefore belongs to rejected input.
                if entry.digits == crate::frequency_entry::count(entry.wide) {
                    entry.value = 0;
                    entry.digits = 0;
                }
                entry.push(value);
                self.entry = Some(entry);
                if entry.digits < crate::frequency_entry::count(entry.wide) {
                    return;
                }
            } else if matches!(event.key, Key::F) {
                if entry.digits != 0 {
                    entry.value /= 10;
                    entry.digits -= 1;
                } else {
                    entry.wide = !entry.wide;
                }
                self.entry = Some(entry);
                return;
            } else if matches!(event.key, Key::Exit) {
                self.entry = None;
                return;
            } else if !matches!(event.key, Key::Menu) {
                return;
            }

            match entry.frequency() {
                Some(frequency) => {
                    self.vfos[self.active].set_frequency(frequency);
                    self.entry = None;
                }
                None => {
                    self.notice = if entry.digits == crate::frequency_entry::count(entry.wide) { 6 } else { 8 };
                }
            }
            return;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.scan.stop();
            let up = matches!(event.key, Key::Up);
            match self.mode {
                Mode::Channel => {
                    self.slot = if up {
                        (self.slot + 1) & 15
                    } else {
                        (self.slot + 15) & 15
                    };
                    if bank.get(self.slot).is_none() { self.notice = 3; }
                }
                Mode::Vfo => {
                    let vfo = &mut self.vfos[self.active];
                    vfo.tune.frequency = vfo.band().shift_receive(
                        vfo.tune.frequency, vfo.tune.step, up,
                    );
                }
            }
            return;
        }

        if !matches!(event.edge, Edge::Down) {
            return;
        }

        match event.key {
            Key::Menu => {
                self.enabled = !self.enabled;
                if !self.enabled { self.scan.stop(); }
            }
            Key::Zero => {
                self.monitor = !self.monitor;
                self.scan.stop();
            }
            Key::One => {
                self.scan.stop();
                if matches!(self.mode, Mode::Vfo) {
                    let tune = &mut self.vfos[self.active].tune;
                    tune.narrow = !tune.narrow;
                }
            }
            Key::Two => {
                self.scan.stop();
                if matches!(self.mode, Mode::Vfo) {
                    let tune = &mut self.vfos[self.active].tune;
                    tune.step = match tune.step {
                        12_500 => 25_000,
                        25_000 => 100_000,
                        _ => 12_500,
                    };
                }
            }
            Key::Three => {
                self.scan.stop();
                self.mode = Mode::Vfo;
                self.vfos[self.active].switch_band();
            }
            Key::Four => {
                self.scan.stop();
                self.mode = match self.mode {
                    Mode::Vfo => Mode::Channel,
                    Mode::Channel => Mode::Vfo,
                };
                if self.tune(bank).is_none() { self.notice = 3; }
            }
            Key::Five => {
                self.scan.stop();
                if matches!(self.mode, Mode::Vfo) {
                    match self.storable_vfo() {
                        Some(tune) => {
                            if bank.store_vfo(self.slot, tune, self.vfos[self.active].tx_signal, self.vfos[self.active].rx_signal) {
                                self.notice = 1;
                            }
                        }
                        None => self.notice = 7,
                    }
                }
            }
            Key::Six => {
                self.scan.stop();
                self.mode = Mode::Vfo;
                self.active ^= 1;
            }
            Key::Seven => {
                self.scan.stop();
                self.slot = (self.slot + 1) & 15;
                self.notice = 2;
            }
            Key::Eight => {
                self.scan.stop();
                self.mode = Mode::Vfo;
                let mut entry = Entry::new();
                entry.wide = self.vfos[self.active].tune.frequency >= 1_000_000_000;
                self.entry = Some(entry);
            }
            Key::Nine => {
                if self.scan.active() {
                    self.scan.stop();
                } else if self.monitor || self.squelch == 0 {
                    self.notice = 5;
                } else if matches!(self.mode, Mode::Channel)
                    && bank.scan_count(self.scan.preferences.skipped) < 2
                {
                    self.notice = 4;
                } else {
                    if matches!(self.mode, Mode::Channel)
                        && (bank.get(self.slot).is_none()
                            || self.scan.preferences.skipped(self.slot))
                    {
                        if let Some(slot) = bank.next_scan(
                            self.slot,
                            self.scan.preferences.skipped,
                            self.scan.preferences.reverse,
                        ) {
                            self.slot = slot;
                        }
                    }
                    self.enabled = true;
                    self.scan.phase = ScanPhase::Wait;
                }
            }
            _ => {}
        }
    }

    pub(super) fn observe(&mut self, now: u32, receiver: View, ptt: bool, bank: &Bank) {
        if !self.scan.active() { return; }
        if ptt || !self.enabled || self.monitor || self.squelch == 0
            || self.editing() || matches!(receiver.phase, Phase::Failed)
        {
            self.scan.stop();
            return;
        }

        let tune = match self.tune(bank) {
            Some(tune) => tune,
            None => {
                self.scan.stop();
                return;
            }
        };
        let ready = matches!(receiver.phase, Phase::Receive)
            && receiver.frequency == tune.frequency
            && receiver.narrow == tune.narrow
            && receiver.am == tune.am
            && receiver.rx_signal == self.rx_signal(bank);

        if !self.scan.poll(now, ready, ready && receiver.signal) {
            return;
        }

        match self.mode {
            Mode::Channel => match bank.next_scan(
                self.slot,
                self.scan.preferences.skipped,
                self.scan.preferences.reverse,
            ) {
                Some(slot) => self.slot = slot,
                None => self.scan.stop(),
            },
            Mode::Vfo => {
                let preferences = self.scan.preferences;
                let vfo = &mut self.vfos[self.active];
                let same_band = crate::profile::receive_span(preferences.lower)
                    == crate::profile::receive_span(vfo.tune.frequency);
                let (path_lower, path_upper) =
                    crate::profile::receive_bounds(vfo.tune.frequency);

                let lower = if same_band {
                    preferences.lower
                } else {
                    path_lower
                };

                let upper = if same_band {
                    preferences.upper
                } else {
                    path_upper
                };

                let current = vfo.tune.frequency;

                vfo.tune.frequency = if current < lower || current > upper {
                    if preferences.reverse { upper } else { lower }
                } else if preferences.reverse {
                    if current <= lower {
                        upper
                    } else {
                        current.saturating_sub(vfo.tune.step).max(lower)
                    }
                } else if current >= upper {
                    lower
                } else {
                    current.saturating_add(vfo.tune.step).min(upper)
                };
            }
        }
    }

    #[inline(never)]
    pub(super) const fn snapshot(&self, bank: &Bank, light: bool, period: u32) -> Snapshot {
        let mut state = Snapshot { words: [0; 148] };
        crate::app::preferences::Preferences::new().write(&mut state.words);
        state.words[108] = crate::app::appearance::Settings::new().word();
        state.words[109] = crate::app::tx_options::DEFAULT_TOT as u32;
        state.words[0] = light as u32;
        state.words[1] = period;
        state.words[2] = self.active as u32;
        state.words[3] = self.slot as u32;
        state.words[4] = self.mode as u32;
        state.words[5] = self.squelch as u32;

        let mut index = 0;
        while index < 2 {
            let at = 8 + index * 4;
            state.words[at] = self.vfos[index].tune.frequency;
            state.words[at + 1] = self.vfos[index].other;
            state.words[at + 2] = self.vfos[index].tune.step;
            state.words[at + 3] = self.vfos[index].tune.flags();
            state.words[130 + index] = crate::ctcss::pair(
                self.vfos[index].tx_signal, self.vfos[index].rx_signal,
            );
            state.words[112 + index] = match self.vfos[index].transmit.word() {
                Ok(word) => word,
                Err(_) => panic!("invalid owned VFO route"),
            };
            index += 1;
        }
        index = 0;
        while index < 16 {
            let at = 16 + index * 5;
            if let Some(channel) = bank.channel(index) {
                state.words[at] = channel.tune.frequency;
                state.words[at + 1] = channel.tune.step;
                state.words[at + 2] = channel.tune.flags();
                state.words[at + 3] = u32::from_le_bytes([
                    channel.name[0], channel.name[1], channel.name[2], channel.name[3],
                ]);
                state.words[at + 4] = u32::from_le_bytes([
                    channel.name[4], channel.name[5], channel.name[6], channel.name[7],
                ]);
                state.words[132 + index] = crate::ctcss::pair(
                    channel.tx_signal, channel.rx_signal,
                );
                state.words[114 + index] = match channel.transmit.word() {
                    Ok(word) => word,
                    Err(_) => panic!("owned channel route rejected"),
                };
            } else {
                assert!(bank.slots[index].empty());
            }
            index += 1;
        }
        state
    }

    /// Validation precedes any state change. Runtime admission starts off:
    /// no stored field can restore RX, monitor, scanning, or an input edit.
    #[inline(never)]
    pub(super) const fn restore(&mut self, bank: &mut Bank, state: &Snapshot) -> bool {
        if !state.valid() { return false; }

        *self = Self::new();
        self.active = state.words[2] as usize;
        self.slot = state.words[3] as usize;
        self.mode = if state.words[4] == 0 { Mode::Vfo } else { Mode::Channel };
        self.squelch = state.words[5] as u8;

        let mut index = 0;
        while index < 2 {
            let at = 8 + index * 4;
            self.vfos[index] = Vfo {
                tune: Tune {
                    frequency: state.words[at],
                    step: state.words[at + 2],
                    am: state.words[at + 3] & 2 != 0, narrow: state.words[at + 3] & 1 != 0,
                },
                other: state.words[at + 1],
                tx_signal: state.words[130 + index] as u16,
                rx_signal: (state.words[130 + index] >> 16) as u16,
                transmit: match crate::profile::Tx::decode(state.words[112 + index]) {
                    Ok(route) => route,
                    Err(_) => panic!("validated VFO route rejected"),
                },
            };
            index += 1;
        }
        index = 0;
        while index < 16 {
            let at = 16 + index * 5;
            let first = state.words[at + 3].to_le_bytes();
            let second = state.words[at + 4].to_le_bytes();
            if state.words[at] == 0 {
                bank.slots[index] = Slot::EMPTY;
            } else {
                let transmit = match crate::profile::Tx::decode(state.words[114 + index]) {
                    Ok(route) => route,
                    Err(_) => panic!("validated channel route rejected"),
                };
                assert!(bank.update(index, Channel {
                    tune: Tune {
                        frequency: state.words[at],
                        step: state.words[at + 1],
                        am: state.words[at + 2] & 2 != 0, narrow: state.words[at + 2] & 1 != 0,
                    },
                    transmit,
                    tx_signal: state.words[132 + index] as u16,
                    rx_signal: (state.words[132 + index] >> 16) as u16,
                    name: [
                        first[0], first[1], first[2], first[3],
                        second[0], second[1], second[2], second[3],
                    ],
                }));
            }
            index += 1;
        }
        true
    }

    pub(super) fn publish(&self, now: u32, bank: &Bank) {
        if !cfg!(feature = "debug-tools") {
            return;
        }

        let tune = self.tune(bank).unwrap_or(self.vfos[self.active].tune);
        let state = ControlState {
            signature: 0,
            tick: now,
            enabled: self.enabled as u32,
            mode: self.mode as u32,
            vfo: self.active as u32,
            slot: self.slot as u32,
            channels: bank.count(),
            scan: self.scan.phase as u32,
            hops: self.scan.hops,
            frequency: tune.frequency,
            other_frequency: self.vfos[self.active].other,
            step: tune.step,
            squelch: self.squelch as u32,
            narrow: tune.narrow as u32,
            monitor: self.monitor as u32,
            editing: self.editing() as u32,
        };
        crate::observation::publish!(TRAKT_CONTROL, state, 0x544F_4354);
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const _: () = {
    let mut operation = Operation::new();
    let mut bank = Bank::new();
    let original = operation.active_vfo().frequency;
    operation.enabled = true;
    operation.handle(press(Key::Eight), &mut bank);

    let invalid = [
        Key::Six, Key::Three, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    let mut index = 0;
    while index < invalid.len() {
        operation.handle(press(invalid[index]), &mut bank);
        assert!(operation.active_vfo().frequency == original);
        assert!(!operation.vfo_request(0).enabled);
        index += 1;
    }
    assert!(operation.editing() && operation.entry_refused());
    assert!(operation.notice == 6);

    // Deletion edits the retained candidate instead of changing the VFO.
    operation.handle(press(Key::F), &mut bank);
    let candidate = match operation.entry {
        Some(value) => value,
        None => panic!("rejected frequency candidate lost"),
    };
    assert!(candidate.digits == 5 && candidate.value == 63000);
    assert!(!operation.entry_refused());
    operation.handle(press(Key::Menu), &mut bank);
    assert!(operation.notice == 8 && operation.entry_refused());

    operation.handle(press(Key::Zero), &mut bank);
    assert!(operation.notice == 6 && operation.entry_refused());

    // A digit after a complete refusal begins a fresh six-digit candidate.
    operation.handle(press(Key::Zero), &mut bank);
    let candidate = match operation.entry {
        Some(value) => value,
        None => panic!("replacement frequency candidate lost"),
    };
    assert!(candidate.digits == 1 && candidate.value == 0);
    let remaining = [Key::Five, Key::Zero, Key::Zero, Key::Zero];
    index = 0;
    while index < remaining.len() {
        operation.handle(press(remaining[index]), &mut bank);
        index += 1;
    }
    assert!(operation.editing());
    assert!(operation.active_vfo().frequency == original);

    operation.handle(Event { key: Key::Zero, edge: Edge::Repeat }, &mut bank);
    assert!(operation.editing());
    operation.handle(press(Key::Zero), &mut bank);
    assert!(!operation.editing());
    assert!(operation.active_vfo().frequency == 50_000_000);
    assert!(operation.vfo_request(0).enabled);

    // Key release cannot reopen entry or change the accepted value.
    operation.handle(Event { key: Key::Zero, edge: Edge::Up }, &mut bank);
    assert!(operation.active_vfo().frequency == 50_000_000);
    assert!(!operation.editing());

    operation.handle(press(Key::Eight), &mut bank);
    operation.handle(press(Key::Six), &mut bank);
    operation.handle(press(Key::Exit), &mut bank);
    assert!(!operation.editing());
    assert!(operation.active_vfo().frequency == 50_000_000);
};

const _: () = {
    use crate::profile::Tx;

    let mut operation = Operation::new();
    let mut bank = Bank::new();
    assert!(operation.set_tx_route(Tx::Minus(600_000)));
    assert!(matches!(operation.transmit_frequency(&bank), Ok(144_900_000)));
    assert!(operation.storable_vfo().is_some());

    operation.handle(press(Key::Five), &mut bank);
    assert!(bank.count() == 1 && operation.notice == 1);
    let stored = match bank.channel(0) {
        Some(channel) => channel,
        None => panic!("stored routed VFO missing"),
    };
    assert!(matches!(stored.transmit, Tx::Minus(600_000)));

    assert!(operation.configure(super::menu::Setting::Vfo, 1));
    assert!(matches!(operation.tx_route(), Some(Tx::Simplex)));
    assert!(operation.set_tx_route(Tx::ReceiveOnly));
    assert!(matches!(operation.transmit_frequency(&bank), Err(0x700C)));

    let state = operation.snapshot(&bank, true, 250);
    assert!(state.valid());
    let mut restored = Operation::new();
    let mut restored_bank = Bank::new();
    assert!(restored.restore(&mut restored_bank, &state));
    assert!(matches!(restored.tx_route(), Some(Tx::ReceiveOnly)));
    assert!(restored.configure(super::menu::Setting::Vfo, 0));
    assert!(matches!(restored.tx_route(), Some(Tx::Minus(600_000))));

    // Tuning cannot turn a blocked offset into simplex.
    restored.vfos[0].set_frequency(136_000_000);
    assert!(matches!(restored.transmit_frequency(&restored_bank), Err(0x700B)));
    let blocked = restored.snapshot(&restored_bank, true, 250);
    assert!(blocked.valid());
    assert!(matches!(restored.tx_route(), Some(Tx::Minus(600_000))));

    assert!(bank.put(0, Tune::initial(Band::Uhf)));
    assert!(operation.configure(super::menu::Setting::Mode, 1));
    assert!(operation.tx_route().is_none());
    assert!(matches!(operation.transmit_frequency(&bank), Ok(433_500_000)));
    assert!(!operation.set_tx_route(Tx::Plus(5_000_000)));
};

const _: () = {
    use crate::profile::Tx;

    let mut bank = Bank::new();
    let channel = Channel {
        tune: Tune { frequency: 145_700_000, step: 12_500, am: false, narrow: false },
        transmit: Tx::Minus(600_000),
        tx_signal: 0,
        rx_signal: 0,
        name: *b"REPEATER",
    };
    assert!(bank.update(0, channel));
    assert!(bank.copy_slot(0, 15));
    assert!(bank.move_slot(15, 7));

    let copied = match bank.channel(7) {
        Some(channel) => channel,
        None => panic!("channel route lost during move"),
    };
    assert!(matches!(copied.transmit, Tx::Minus(600_000)));
    assert!(copied.name[0] == b'R');
    assert!(bank.channel(15).is_none());

    let mut bytes = [0u8; 320];
    bank.export_wire(&mut bytes);
    assert!(matches!(Bank::validate_wire(&bytes, 2), Some(2)));
    let mut imported = Bank::new();
    assert!(imported.import_wire(&bytes, 2));

    let mut operation = Operation::new();
    assert!(operation.configure(super::menu::Setting::Mode, 1));
    assert!(matches!(operation.transmit_frequency(&imported), Ok(145_100_000)));

    let mut rx_only = copied;
    rx_only.transmit = Tx::ReceiveOnly;
    assert!(imported.update(7, rx_only));
    assert!(operation.configure(super::menu::Setting::Slot, 7));
    assert!(matches!(operation.transmit_frequency(&imported), Err(0x700C)));

    let mut before = [0u8; 320];
    imported.export_wire(&mut before);

    // A malformed final empty slot rejects the entire incoming bank.
    bytes[319] = b' ';
    assert!(!imported.import_wire(&bytes, 2));
    let mut after = [0u8; 320];
    imported.export_wire(&mut after);
    let mut index = 0;
    while index < before.len() {
        assert!(before[index] == after[index]);
        index += 1;
    }

    assert!(imported.delete(7));
    let snapshot = operation.snapshot(&imported, true, 250);
    assert!(snapshot.valid());
    assert!(snapshot.words[114 + 7] == 0);
};

const _: () = {
    assert!(core::mem::size_of::<Bank>() == 320);
    assert!(core::mem::size_of::<Slot>() == 20);
    assert!(Bank::capacity() == 16);
    assert!(Bank::slot_bytes() == 20);
    assert!(core::mem::size_of::<Bank>() == Bank::capacity() * Bank::slot_bytes());

    let mut full = Bank::new();
    let mut slot = 0;
    while slot < Bank::capacity() {
        assert!(full.put(slot, Tune {
            frequency: 145_000_000 + slot as u32 * 12_500,
            step: 12_500,
            am: false, narrow: slot & 1 != 0,
        }));
        slot += 1;
    }
    assert!(full.count() == 16);
    slot = 0;
    while slot < Bank::capacity() {
        match full.get(slot) {
            Some(tune) => {
                assert!(tune.frequency == 145_000_000 + slot as u32 * 12_500);
                assert!(tune.narrow == (slot & 1 != 0));
            }
            None => panic!("filled channel missing"),
        }
        slot += 1;
    }
    full.clear();
    assert!(full.count() == 0);
    assert!(core::mem::size_of::<ControlState>() == 64);

    let mut scan_bank = Bank::new();
    assert!(scan_bank.put(0, Tune::initial(Band::Vhf)));
    assert!(scan_bank.put(3, Tune::initial(Band::Uhf)));
    assert!(scan_bank.put(15, Tune::initial(Band::Vhf)));

    assert!(scan_bank.scan_count(0) == 3);
    assert!(scan_bank.scan_count(1) == 2);
    assert!(scan_bank.scan_count(0x8009) == 0);

    assert!(matches!(scan_bank.next_scan(0, 0, false), Some(3)));
    assert!(matches!(scan_bank.next_scan(0, 0, true), Some(15)));
    assert!(matches!(scan_bank.next_scan(0, 8, false), Some(15)));
    assert!(scan_bank.next_scan(0, 0x8009, false).is_none());

    let mut timed_scan = Scan::new();
    timed_scan.preferences.resume = 1;
    timed_scan.phase = ScanPhase::Wait;

    assert!(!timed_scan.poll(0, true, false));
    assert!(!timed_scan.poll(100, true, true));
    assert!(!timed_scan.poll(299, true, true));
    assert!(timed_scan.poll(300, true, true));

    let mut stop_scan = Scan::new();
    stop_scan.preferences.resume = 2;
    stop_scan.phase = ScanPhase::Wait;

    assert!(!stop_scan.poll(0, true, false));
    assert!(!stop_scan.poll(100, true, true));
    assert!(!stop_scan.active());

    let mut settings = Operation::new();
    assert!(settings.configure(super::menu::Setting::Sql, 9));
    assert!(settings.squelch == 9);
    assert!(!settings.configure(super::menu::Setting::Sql, 10));
    assert!(settings.squelch == 9);
    assert!(settings.configure(super::menu::Setting::Vfo, 1));
    assert!(settings.active == 1);
    assert!(settings.configure(super::menu::Setting::Band, 0));
    assert!(settings.vfos[1].tune.frequency == 145_500_000);
    assert!(settings.configure(super::menu::Setting::Mode, 1));
    assert!(!settings.configure(super::menu::Setting::Width, 1));
    assert!(!settings.configure(super::menu::Setting::Step, 2));
    assert!(settings.configure(super::menu::Setting::Slot, 15));
    assert!(!settings.configure(super::menu::Setting::Slot, 16));

    let mut bank = Bank::new();
    let mut operation = Operation::new();
    assert!(bank.count() == 0);

    operation.handle(press(Key::Five), &mut bank);
    assert!(bank.count() == 1);
    operation.handle(press(Key::Seven), &mut bank);
    operation.handle(press(Key::Six), &mut bank);
    assert!(operation.active == 1);
    operation.handle(press(Key::Five), &mut bank);
    assert!(bank.count() == 2);

    let first = match bank.get(0) {
        Some(tune) => tune,
        None => panic!("VHF channel missing"),
    };
    let second = match bank.get(1) {
        Some(tune) => tune,
        None => panic!("UHF channel missing"),
    };
    assert!(first.frequency == 145_500_000);
    assert!(second.frequency == 433_500_000);
    assert!(matches!(bank.next(0), Some(1)));
    assert!(matches!(bank.next(1), Some(0)));

    operation.handle(press(Key::Four), &mut bank);
    assert!(matches!(operation.mode, Mode::Channel));
    operation.handle(press(Key::Nine), &mut bank);
    assert!(operation.scan.active());
    operation.handle(press(Key::Zero), &mut bank);
    assert!(!operation.scan.active());
    operation.handle(press(Key::Nine), &mut bank);
    assert!(!operation.scan.active());
    operation.handle(press(Key::Zero), &mut bank);
    operation.handle(press(Key::Nine), &mut bank);
    assert!(operation.scan.active());
    operation.cycle_sql();
    assert!(!operation.scan.active());

    operation.handle(press(Key::Eight), &mut bank);
    let digits = [Key::One, Key::Four, Key::Five, Key::Five, Key::Zero, Key::Zero];
    let mut index = 0;
    while index < digits.len() {
        operation.handle(press(digits[index]), &mut bank);
        index += 1;
    }
    assert!(!operation.editing());
    assert!(operation.vfos[1].tune.frequency == 145_500_000);
    assert!(operation.vfos[1].other == 433_500_000);
    operation.handle(press(Key::Three), &mut bank);
    assert!(operation.vfos[1].tune.frequency == 433_500_000);

    let mut entry = Entry::new();
    entry.push(4);
    entry.push(3);
    entry.push(3);
    entry.push(5);
    entry.push(0);
    entry.push(0);
    entry.push(9);
    assert!(matches!(entry.frequency(), Some(433_500_000)));
    let preview = entry.preview();
    assert!(preview[0] == b'4' && preview[3] == b'.' && preview[6] == b'0');

    let mut invalid = Entry::new();
    index = 0;
    while index < 6 {
        invalid.push(7);
        index += 1;
    }
    assert!(invalid.value == 777777 && invalid.digits == 6);
    assert!(invalid.frequency().is_none());

    let mut upper_six = Entry::new();
    index = 0;
    while index < 6 {
        upper_six.push(9);
        if index < 5 {
            assert!(upper_six.frequency().is_none());
        }
        index += 1;
    }
    assert!(matches!(upper_six.frequency(), Some(999_999_000)));

    let mut scan = Scan::new();
    scan.phase = ScanPhase::Wait;
    assert!(!scan.poll(0, false, false));
    assert!(!scan.poll(50, true, false));
    assert!(!scan.poll(349, true, false));
    assert!(scan.poll(350, true, false));
    assert!(!scan.poll(351, false, false));
    assert!(!scan.poll(400, true, false));
    assert!(!scan.poll(420, true, true));
    assert!(!scan.poll(600, true, true));
    assert!(!scan.poll(1_099, true, false));
    assert!(scan.poll(1_100, true, false));

    let mut wrap = Scan::new();
    wrap.phase = ScanPhase::Wait;
    assert!(!wrap.poll(u32::MAX - 100, true, false));
    assert!(!wrap.poll(198, true, false));
    assert!(wrap.poll(199, true, false));

    let mut empty = Bank::new();
    let mut rejected = Operation::new();
    rejected.handle(press(Key::Four), &mut empty);
    rejected.handle(press(Key::Nine), &mut empty);
    assert!(!rejected.scan.active());
    assert!(!empty.put(16, first));
    assert!(!empty.put(0, Tune {
        frequency: 630_000_000, step: 12_500, am: false, narrow: false,
    }));

    let state = operation.snapshot(&bank, false, 500);
    assert!(state.valid());

    let mut restored = Operation::new();
    let mut restored_bank = Bank::new();
    restored.enabled = true;
    restored.monitor = true;
    restored.scan.phase = ScanPhase::Hold;
    assert!(restored.restore(&mut restored_bank, &state));
    assert!(!restored.enabled && !restored.monitor && !restored.scan.active());
    assert!(restored.entry.is_none());
    assert!(restored.active == operation.active);
    assert!(restored.slot == operation.slot);
    assert!(restored.squelch == operation.squelch);
    assert!(restored_bank.count() == 2);

    let roundtrip = restored.snapshot(&restored_bank, false, 500);
    index = 0;
    while index < 148 {
        assert!(roundtrip.words[index] == state.words[index]);
        index += 1;
    }

    let mut bad = state;
    bad.words[10] = 0;
    assert!(!restored.restore(&mut restored_bank, &bad));
    assert!(restored_bank.count() == 2);
    let named = Channel {
        tune: first,
        transmit: crate::profile::Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: *b"LOCAL   ",
    };
    assert!(named.valid() && named.named());
    assert!(restored_bank.update(0, named));
    assert!(!restored_bank.update(16, named));

    let replaced = Tune {
        frequency: 145_525_000,
        step: 25_000,
        am: false, narrow: true,
    };
    assert!(restored_bank.put(0, replaced));
    let retained_name = match restored_bank.channel(0) {
        Some(channel) => channel,
        None => panic!("named channel lost"),
    };
    assert!(retained_name.name[0] == b'L');
    assert!(retained_name.name[4] == b'L');
    assert!(retained_name.tune.frequency == 145_525_000);

    let named_state = restored.snapshot(&restored_bank, false, 500);
    let mut named_bank = Bank::new();
    let mut named_operation = Operation::new();
    assert!(named_operation.restore(&mut named_bank, &named_state));
    let imported = match named_bank.channel(0) {
        Some(channel) => channel,
        None => panic!("named snapshot lost"),
    };
    index = 0;
    while index < 8 {
        assert!(imported.name[index] == named.name[index]);
        index += 1;
    }

    let mut invalid_name = named;
    invalid_name.name[0] = 0;
    assert!(!named_bank.update(0, invalid_name));
    assert!(named_bank.count() == 2);
    assert!(named_bank.delete(0));
    assert!(!named_bank.delete(0));
    assert!(!named_bank.delete(16));
    let deleted_state = named_operation.snapshot(&named_bank, false, 500);
    assert!(deleted_state.valid());
    assert!(deleted_state.words[16] == 0);
    assert!(deleted_state.words[19] == 0);
    assert!(deleted_state.words[20] == 0);

    restored_bank.clear();
    assert!(restored_bank.count() == 0);

    let alpha = Channel {
        tune: Tune {
            frequency: 145_500_000,
            step: 12_500,
            am: false, narrow: false,
        },
        transmit: crate::profile::Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: *b"ALPHA   ",
    };

    let bravo = Channel {
        tune: Tune {
            frequency: 433_700_000,
            step: 25_000,
            am: false, narrow: true,
        },
        transmit: crate::profile::Tx::Simplex,
        tx_signal: 0,
        rx_signal: 0,
        name: *b"BRAVO   ",
    };

    let mut managed = Bank::new();
    assert!(managed.update(0, alpha));
    assert!(managed.update(1, bravo));

    assert!(managed.copy_slot(0, 2));
    assert!(managed.count() == 3);

    let copied = match managed.channel(2) {
        Some(channel) => channel,
        None => panic!("copied channel missing"),
    };

    assert!(copied.tune.frequency == alpha.tune.frequency);
    assert!(copied.name[0] == b'A');
    assert!(managed.channel(0).is_some());

    assert!(managed.move_slot(2, 3));
    assert!(managed.channel(2).is_none());
    assert!(managed.channel(3).is_some());
    assert!(managed.count() == 3);

    assert!(managed.move_slot(0, 1));

    let first_swapped = match managed.channel(0) {
        Some(channel) => channel,
        None => panic!("exchange lost destination"),
    };

    let second_swapped = match managed.channel(1) {
        Some(channel) => channel,
        None => panic!("exchange lost source"),
    };

    assert!(first_swapped.tune.frequency == bravo.tune.frequency);
    assert!(first_swapped.name[0] == b'B');
    assert!(second_swapped.tune.frequency == alpha.tune.frequency);
    assert!(second_swapped.name[0] == b'A');
    assert!(managed.count() == 3);

    assert!(managed.copy_slot(1, 0));

    let overwritten = match managed.channel(0) {
        Some(channel) => channel,
        None => panic!("replacement missing"),
    };

    assert!(overwritten.tune.frequency == alpha.tune.frequency);
    assert!(overwritten.name[0] == b'A');
    assert!(managed.channel(1).is_some());

    assert!(!managed.copy_slot(1, 1));
    assert!(!managed.copy_slot(16, 0));
    assert!(!managed.copy_slot(0, 16));
    assert!(!managed.copy_slot(2, 0));

    assert!(!managed.move_slot(0, 0));
    assert!(!managed.move_slot(16, 0));
    assert!(!managed.move_slot(0, 16));
    assert!(!managed.move_slot(2, 0));

    assert!(managed.count() == 3);

    assert!(managed.store_vfo(0, crate::profile::Profile::simplex(bravo.tune), 0, 0));
    let stored = match managed.channel(0) {
        Some(channel) => channel,
        None => panic!("stored VFO missing"),
    };

    assert!(stored.tune.frequency == bravo.tune.frequency);
    assert!(stored.name[0] == b'A');
    assert!(!managed.store_vfo(16, crate::profile::Profile::simplex(bravo.tune), 0, 0));
};
const _: () = {
    use super::menu::Setting;
    let mut operation = Operation::new();
    let mut bank = Bank::new();
    assert!(operation.set_vfo_rx_signal(885));
    assert!(operation.configure(Setting::Demod, 1));
    operation.vfos[0].set_frequency(118_025_000);
    assert!(operation.active_vfo().am);
    assert!(operation.vfo_rx_signal() == 885);
    assert!(operation.rx_signal(&bank) == 0);
    assert!(operation.vfo_request(0).am);
    assert!(operation.vfo_request(0).rx_signal == 0);
    assert!(matches!(operation.transmit_frequency(&bank), Err(0x700F)));

    assert!(operation.set_tx_route(crate::profile::Tx::Split(145_500_000)));
    assert!(matches!(operation.transmit_frequency(&bank), Err(0x700F)));
    assert!(bank.store_vfo(
        0, operation.active_profile(), operation.vfo_signal(), operation.vfo_rx_signal(),
    ));

    let state = operation.snapshot(&bank, true, 250);
    assert!(state.valid());
    let mut restored = Operation::new();
    let mut restored_bank = Bank::new();
    assert!(restored.restore(&mut restored_bank, &state));
    assert!(restored.active_vfo().am);
    assert!(restored.vfo_rx_signal() == 885);
    assert!(restored.configure(Setting::Mode, 1));
    assert!(matches!(restored.transmit_frequency(&restored_bank), Err(0x700F)));
    assert!(!restored.configure(Setting::Demod, 0));

    assert!(operation.configure(Setting::Demod, 0));
    assert!(operation.rx_signal(&bank) == 885);
    assert!(matches!(operation.transmit_frequency(&bank), Ok(145_500_000)));
};

const _: () = {
    use super::menu::Setting;

    let mut operation = Operation::new();
    let mut bank = Bank::new();
    operation.vfos[0].set_frequency(250_000_000);
    assert!(operation.vfos[0].other == 433_500_000);
    operation.vfos[0].set_frequency(350_000_000);
    assert!(operation.vfos[0].other == 250_000_000);
    assert!(operation.configure(Setting::Band, 0));
    assert!(operation.active_vfo().frequency == 250_000_000);
    assert!(operation.configure(Setting::Band, 1));
    assert!(operation.active_vfo().frequency == 350_000_000);
    assert!(matches!(operation.transmit_frequency(&bank), Err(0x700B)));

    assert!(operation.set_tx_route(crate::profile::Tx::ReceiveOnly));
    assert!(bank.store_vfo(0, operation.active_profile(), 0, 0));
    let state = operation.snapshot(&bank, true, 250);
    assert!(state.valid());

    let mut restored = Operation::new();
    let mut copied = Bank::new();
    assert!(restored.restore(&mut copied, &state));
    assert!(restored.active_vfo().frequency == 350_000_000);
    assert!(restored.configure(Setting::Band, 0));
    assert!(restored.active_vfo().frequency == 250_000_000);
    assert!(matches!(restored.transmit_frequency(&copied), Err(0x700C)));

    let mut entry = Entry::new();
    let digits = [0, 5, 0, 0, 0, 0];
    let mut index = 0;
    while index < digits.len() {
        entry.push(digits[index]);
        index += 1;
    }
    assert!(matches!(entry.frequency(), Some(50_000_000)));

    let mut upper = Entry::new();
    let digits = [6, 2, 9, 9, 9, 9];
    index = 0;
    while index < digits.len() {
        upper.push(digits[index]);
        index += 1;
    }
    assert!(matches!(upper.frequency(), Some(629_999_000)));
};

const _: () = {
    let mut operation = Operation::new();
    let mut bank = Bank::new();
    operation.handle(press(Key::Eight), &mut bank);
    operation.handle(press(Key::F), &mut bank);
    assert!(matches!(operation.entry, Some(entry) if entry.wide));

    let digits = [
        Key::One, Key::Zero, Key::Zero, Key::Zero,
        Key::Zero, Key::Zero, Key::Zero,
    ];
    let mut index = 0;
    while index < digits.len() {
        operation.handle(press(digits[index]), &mut bank);
        if index < 6 {
            assert!(operation.editing());
            assert!(operation.active_vfo().frequency == 145_500_000);
        }
        index += 1;
    }
    assert!(!operation.editing());
    assert!(operation.active_vfo().frequency == 1_000_000_000);
    assert!(operation.vfos[0].other == 145_500_000);

    operation.handle(press(Key::Eight), &mut bank);
    assert!(matches!(operation.entry, Some(entry) if entry.wide));
    index = 0;
    while index < 7 {
        operation.handle(press(Key::Nine), &mut bank);
        index += 1;
    }
    assert!(operation.entry_refused());
    assert!(operation.active_vfo().frequency == 1_000_000_000);
    operation.handle(press(Key::Exit), &mut bank);

    operation.vfos[0].switch_band();
    assert!(operation.active_vfo().frequency == 145_500_000);
    operation.vfos[0].switch_band();
    assert!(operation.active_vfo().frequency == 1_000_000_000);
};

const _: () = {
    let mut operation = Operation::new();
    let bank = Bank::new();
    let mut settings = crate::slope::Settings::new();
    settings.enabled = true;
    operation.vfos[0].tune.am = true;
    let before = operation.snapshot(&bank, true, 250);
    let actual = operation.transmit_with(&bank, settings);

    if cfg!(feature = "fam") {
        assert!(matches!(actual, Ok(145_495_000)));
    } else {
        assert!(matches!(actual, Err(0x700F)));
    }
    assert!(matches!(operation.transmit_frequency(&bank), Err(0x700F)));

    let after = operation.snapshot(&bank, true, 250);
    let mut index = 0;
    while index < before.words.len() {
        assert!(before.words[index] == after.words[index]);
        index += 1;
    }
};
