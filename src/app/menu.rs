//! Fixed menu catalog, navigation, and pending edits.
//!
//! An Item describes a label, a short explanation, and either a setting,
//! action, or confirmed storage operation. Adding an entry changes a table;
//! it does not add another key-dispatch branch or allocate a runtime node.
//!
//! Up and Down wrap within the current page. Each category retains its
//! cursor while the menu is open. Editing stores a candidate locally;
//! Menu emits Set, while Exit discards the candidate without touching state.
//!
//! Storage commands require a separate confirmation screen. A key repeat
//! cannot confirm one. PTT and held keys are never interpreted as selections.
//! VFO-only settings remain visible in channel mode but cannot be edited.

use crate::drivers::keyboard::{Edge, Event, Key};
use crate::storage::journal::Action as FileAction;
use crate::ui::{font::Size, widgets, Frame};

#[derive(Clone, Copy)]
pub(super) enum Setting {
    Rx, Vfo, Mode, Band, Width, Demod, Step, Sql, Monitor, Light, Refresh, Slot,
    Roger, Permit, BootTone, Power, Ham, Tot, DualWatch,
    Autosave, ScanDwell, ScanHold, ScanResume, ScanDirection, ScanSkip,
    SpectrumEnabled, SpectrumStep, SpectrumRssi, SpectrumNoise,
    HomeLayout, HomePrimary, HomeDetail, HomeMeter, BatteryDisplay,
    MenuStyle, UiBeep, TxVisual, UiHints,
    SlopeEnabled, SlopeSide, SlopeDelta, SlopeWidth,
}

#[derive(Clone, Copy)]
pub(super) enum Action {
    Frequency, Scan, SaveChannel, EditChannel, DeleteChannel, ChannelList,
    System, Input, Memory, Storage, Help,
    Stacks, LastFault, Power, Radio, Runtime, Layout, About, Serial,
    ScanRange, Spectrum, TxRoute, TxSignal, RxSignal,
}

#[derive(Clone, Copy)]
enum Kind {
    Page(Group),
    Setting(Setting),
    Action(Action),
    Local(Action),
    File(FileAction),
    Test(crate::debug::Test),
}

#[derive(Clone, Copy)]
enum Confirmation {
    Local(Action),
    File(FileAction),
    Test(crate::debug::Test),
}

#[derive(Clone, Copy)]
struct Item {
    label: &'static [u8],
    hint: &'static [u8],
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Group {
    Radio, Channels, Display, Storage, Tools, Audio, Scan, Vfo, System, Slope,
}

// About and Controls belong to System in every composition. Tools is
// present only when diagnostic navigation is selected; no empty page is shown.
const GROUPS: &[Group] = if cfg!(feature = "diagnostics") {
    &[
        Group::Audio, Group::Channels, Group::Display, Group::Radio,
        Group::Scan, Group::Storage, Group::System, Group::Tools,
    ]
} else {
    &[
        Group::Audio, Group::Channels, Group::Display, Group::Radio,
        Group::Scan, Group::Storage, Group::System,
    ]
};

const SCAN_ALL: [Item; 12] = [
    Item {
        label: b"Range",
        hint: b"Two checked limits in one band",
        kind: Kind::Action(Action::ScanRange),
    },
    Item {
        label: b"Scope",
        hint: b"Sequential muted RSSI overview",
        kind: Kind::Action(Action::Spectrum),
    },
    Item {
        label: b"Spectrum",
        hint: b"Enable the overview mode",
        kind: Kind::Setting(Setting::SpectrumEnabled),
    },
    Item {
        label: b"Scope step",
        hint: b"Spacing of 64 overview points",
        kind: Kind::Setting(Setting::SpectrumStep),
    },
    Item {
        label: b"Scope RSSI",
        hint: b"Independent marker threshold",
        kind: Kind::Setting(Setting::SpectrumRssi),
    },
    Item {
        label: b"Scope noise",
        hint: b"Maximum noise for a marker",
        kind: Kind::Setting(Setting::SpectrumNoise),
    },
    Item {
        label: b"Start / stop",
        hint: b"Carrier scan, not spectrum",
        kind: Kind::Action(Action::Scan),
    },
    Item {
        label: b"Dwell",
        hint: b"Observe each settled frequency",
        kind: Kind::Setting(Setting::ScanDwell),
    },
    Item {
        label: b"Hold",
        hint: b"Delay after carrier closes",
        kind: Kind::Setting(Setting::ScanHold),
    },
    Item {
        label: b"Resume",
        hint: b"Carrier, timed, or stop",
        kind: Kind::Setting(Setting::ScanResume),
    },
    Item {
        label: b"Direction",
        hint: b"Ascending or descending",
        kind: Kind::Setting(Setting::ScanDirection),
    },
    Item {
        label: b"Skip slot",
        hint: b"Exclude the selected channel",
        kind: Kind::Setting(Setting::ScanSkip),
    },
];

// Range and carrier-scan controls remain available without Spectrum.
// Hidden module preferences stay in the serialized settings snapshot.
const SCAN: &[Item] = if cfg!(feature = "spectrum") {
    &SCAN_ALL
} else {
    &[
        SCAN_ALL[0],
        SCAN_ALL[6],
        SCAN_ALL[7],
        SCAN_ALL[8],
        SCAN_ALL[9],
        SCAN_ALL[10],
        SCAN_ALL[11],
    ]
};

const AUDIO: [Item; 4] = [
    Item {
        label: b"UI beep",
        hint: b"Keys, actions, all, or off",
        kind: Kind::Setting(Setting::UiBeep),
    },
    Item {
        label: b"Roger",
        hint: b"On-air tail after normal release",
        kind: Kind::Setting(Setting::Roger),
    },
    Item {
        label: b"Permit",
        hint: b"Local cue before PA admission",
        kind: Kind::Setting(Setting::Permit),
    },
    Item {
        label: b"Boot tone",
        hint: b"Local ascending startup cue",
        kind: Kind::Setting(Setting::BootTone),
    },
];

const RADIO_ALL: [Item; 10] = [
    Item {
        label: b"SlopeCast",
        hint: b"AM RX with optional shifted FM TX",
        kind: Kind::Page(Group::Slope),
    },
    Item {
        label: b"Dual Watch",
        hint: b"Saved A/B watch; selected TX",
        kind: Kind::Setting(Setting::DualWatch),
    },
    Item { label: b"VFO settings", hint: b"Edit active VFO profile", kind: Kind::Page(Group::Vfo) },
    Item { label: b"Receive", hint: b"Start or stop the receiver", kind: Kind::Setting(Setting::Rx) },
    Item { label: b"Mode", hint: b"VFO tuning or saved channels", kind: Kind::Setting(Setting::Mode) },
    Item { label: b"Squelch", hint: b"Carrier detector threshold", kind: Kind::Setting(Setting::Sql) },
    Item { label: b"Monitor", hint: b"Audio bypass, not TX admission", kind: Kind::Setting(Setting::Monitor) },

    Item { label: b"Power", hint: b"Factory profiles, not watts", kind: Kind::Setting(Setting::Power) },
    Item { label: b"Timeout", hint: b"Total attempt duration, no OFF", kind: Kind::Setting(Setting::Tot) },
    Item { label: b"HAM mode", hint: b"Saved TX filter; RX unchanged", kind: Kind::Setting(Setting::Ham) },
];

// The feature selects navigation and RF admission, not persistent vocabulary.
const RADIO: &[Item] = if cfg!(feature = "fam") {
    &RADIO_ALL
} else {
    RADIO_ALL.split_at(1).1
};

const SLOPE: [Item; 4] = [
    Item {
        label: b"Enable",
        hint: b"Simplex AM only; stored RX stays",
        kind: Kind::Setting(Setting::SlopeEnabled),
    },
    Item {
        label: b"Side",
        hint: b"Actual TX below or above nominal",
        kind: Kind::Setting(Setting::SlopeSide),
    },
    Item {
        label: b"Delta kHz",
        hint: b"F selects 1.0 or 0.1 kHz step",
        kind: Kind::Setting(Setting::SlopeDelta),
    },
    Item {
        label: b"TX width",
        hint: b"FM preset, not measured deviation",
        kind: Kind::Setting(Setting::SlopeWidth),
    },
];

const VFO: [Item; 9] = [
    Item { label: b"RX signal", hint: b"Off, CTCSS, DCS normal or inverse", kind: Kind::Action(Action::RxSignal) },
    Item { label: b"Frequency", hint: b"RX kHz; empty F selects 6 or 7", kind: Kind::Action(Action::Frequency) },
    Item { label: b"TX route", hint: b"Simplex, offset, split, RX only", kind: Kind::Action(Action::TxRoute) },
    Item { label: b"Active VFO", hint: b"Select independent A or B profile", kind: Kind::Setting(Setting::Vfo) },
    Item { label: b"Band", hint: b"Recall other RX path position", kind: Kind::Setting(Setting::Band) },
    Item { label: b"Modulation", hint: b"FM or receive-only AM", kind: Kind::Setting(Setting::Demod) },
    Item { label: b"Width", hint: b"Wide or narrow receiver preset", kind: Kind::Setting(Setting::Width) },
    Item { label: b"Step", hint: b"RX tuning increment", kind: Kind::Setting(Setting::Step) },
    Item { label: b"TX signal", hint: b"Off, CTCSS, DCS normal or inverse", kind: Kind::Action(Action::TxSignal) },
];

const CHANNELS: [Item; 7] = [
    Item {
        label: b"List",
        hint: b"Browse, copy, move, and edit",
        kind: Kind::Action(Action::ChannelList),
    },
    Item { label: b"Mode", hint: b"VFO or channel selection", kind: Kind::Setting(Setting::Mode) },
    Item { label: b"Slot", hint: b"Channel or save destination", kind: Kind::Setting(Setting::Slot) },
    Item { label: b"Store VFO", hint: b"Replace slot; existing name stays", kind: Kind::Local(Action::SaveChannel) },
    Item { label: b"Edit", hint: b"Name, frequency, width, and step", kind: Kind::Action(Action::EditChannel) },
    Item { label: b"Delete", hint: b"Clear this RAM slot, not Flash", kind: Kind::Local(Action::DeleteChannel) },
    Item { label: b"Scan", hint: b"Channel scan needs two entries", kind: Kind::Action(Action::Scan) },
];

const DISPLAY_ALL: [Item; 11] = [
    Item {
        label: b"Layout",
        hint: b"Clean, panel, or detail",
        kind: Kind::Setting(Setting::HomeLayout),
    },
    Item {
        label: b"Primary",
        hint: b"Large frequency or channel name",
        kind: Kind::Setting(Setting::HomePrimary),
    },
    Item {
        label: b"Home info",
        hint: b"Select the secondary data row",
        kind: Kind::Setting(Setting::HomeDetail),
    },
    Item {
        label: b"Meter",
        hint: b"Raw RSSI bar or segments",
        kind: Kind::Setting(Setting::HomeMeter),
    },
    Item {
        label: b"Battery",
        hint: b"Icon, voltage, or hidden",
        kind: Kind::Setting(Setting::BatteryDisplay),
    },
    Item {
        label: b"Menu style",
        hint: b"Cursor, inverse, or outline",
        kind: Kind::Setting(Setting::MenuStyle),
    },
    Item {
        label: b"TX meter",
        hint: b"Amplitude, not deviation",
        kind: Kind::Setting(Setting::TxVisual),
    },
    Item {
        label: b"Hints",
        hint: b"Optional explanations and tips",
        kind: Kind::Setting(Setting::UiHints),
    },
    Item { label: b"Backlight", hint: b"Side1 also toggles the light", kind: Kind::Setting(Setting::Light) },
    Item { label: b"Refresh", hint: b"Screen update interval", kind: Kind::Setting(Setting::Refresh) },
    Item { label: b"Diagnostics", hint: b"Clock and timing counters", kind: Kind::Action(Action::System) },
];

// The diagnostic navigation entry is the final display item.
// Saved presentation fields keep the same meanings in both variants.
const DISPLAY: &[Item] = if cfg!(feature = "debug-tools") {
    &DISPLAY_ALL
} else {
    DISPLAY_ALL.split_at(10).0
};

const STORAGE: [Item; 6] = [
    Item {
        label: b"Autosave",
        hint: b"Explicit, warning, or delayed Save",
        kind: Kind::Setting(Setting::Autosave),
    },
    Item { label: b"Save", hint: b"Commit settings and channels", kind: Kind::File(FileAction::Save) },
    Item { label: b"Load", hint: b"Restore settings and enable RX", kind: Kind::File(FileAction::Load) },
    Item { label: b"Reset RAM", hint: b"Saved records stay untouched", kind: Kind::File(FileAction::ResetRam) },
    Item { label: b"Test torn", hint: b"Write body without commit", kind: Kind::File(FileAction::TestTorn) },
    Item { label: b"Status", hint: b"Generations and I/O diagnostics", kind: Kind::Action(Action::Storage) },
];

const TOOLS_ALL: [Item; 15] = [
    Item { label: b"System", hint: b"Clock and foreground counters", kind: Kind::Action(Action::System) },
    Item { label: b"Inputs", hint: b"Keys, PTT, and battery ADC", kind: Kind::Action(Action::Input) },
    Item { label: b"Calibration", hint: b"Flash identity and CRC", kind: Kind::Action(Action::Memory) },
    Item { label: b"Controls", hint: b"Main-screen key reference", kind: Kind::Action(Action::Help) },
    Item { label: b"Stack", hint: b"Observed use, not worst case", kind: Kind::Action(Action::Stacks) },
    Item { label: b"Last fault", hint: b"Checked retained failure record", kind: Kind::Action(Action::LastFault) },
    Item { label: b"Test fault", hint: b"RF off, exception, then reset", kind: Kind::Test(crate::debug::Test::Fault) },
    Item { label: b"Test watchdog", hint: b"RF off, stop service, then reset", kind: Kind::Test(crate::debug::Test::Watchdog) },
    Item { label: b"Test RAM", hint: b"Restart app without MCU reset", kind: Kind::Test(crate::debug::Test::Reentry) },
    Item { label: b"Power", hint: b"Factory-scaled battery estimate", kind: Kind::Action(Action::Power) },
    Item { label: b"Receiver", hint: b"Raw RSSI, noise, and glitch", kind: Kind::Action(Action::Radio) },
    Item { label: b"Runtime", hint: b"Foreground ticks, not WCET", kind: Kind::Action(Action::Runtime) },
    Item { label: b"Memory", hint: b"Linked RAM and fixed channels", kind: Kind::Action(Action::Layout) },
    Item { label: b"About", hint: b"Embedded version and build name", kind: Kind::Action(Action::About) },
    Item { label: b"UART", hint: b"Buffered input and screenshots", kind: Kind::Action(Action::Serial) },
];

// These entries require no optional diagnostic snapshots.
const SYSTEM: [Item; 2] = [TOOLS_ALL[3], TOOLS_ALL[13]];

// The normal diagnostic page retains read-only operational observations.
// UART remains the last entry so its absence removes only that endpoint.
const TOOLS_BASIC: [Item; 4] = [
    TOOLS_ALL[4], TOOLS_ALL[9], TOOLS_ALL[5], TOOLS_ALL[14],
];

// Extended diagnostics retain the common observations and recovery tests.
// About and Controls are not duplicated here; they remain in System.
const TOOLS_DEBUG: [Item; 13] = [
    TOOLS_ALL[0], TOOLS_ALL[1], TOOLS_ALL[2],
    TOOLS_ALL[4], TOOLS_ALL[5], TOOLS_ALL[6],
    TOOLS_ALL[7], TOOLS_ALL[8], TOOLS_ALL[9],
    TOOLS_ALL[10], TOOLS_ALL[11], TOOLS_ALL[12], TOOLS_ALL[14],
];

const TOOLS: &[Item] = if cfg!(feature = "debug-tools") {
    if cfg!(feature = "uart") {
        &TOOLS_DEBUG
    } else {
        TOOLS_DEBUG.split_at(12).0
    }
} else if cfg!(feature = "diagnostics") {
    if cfg!(feature = "uart") {
        &TOOLS_BASIC
    } else {
        TOOLS_BASIC.split_at(3).0
    }
} else {
    &[]
};

/// Navigation screens are listed before settings and immediate operations.
/// Labels within each rank use ASCII case-insensitive ordering.
const fn navigation(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Page(_) | Kind::Action(
            Action::ChannelList
                | Action::EditChannel
                | Action::System
                | Action::Input
                | Action::Memory
                | Action::Storage
                | Action::Help
                | Action::Stacks
                | Action::LastFault
                | Action::Power
                | Action::Radio
                | Action::Runtime
                | Action::Layout
                | Action::About
                | Action::Serial
                | Action::ScanRange
                | Action::Spectrum
                | Action::TxRoute
                | Action::TxSignal
                | Action::RxSignal
        )
    )
}

const fn folded(byte: u8) -> u8 {
    if byte >= b'a' && byte <= b'z' {
        byte - 32
    } else {
        byte
    }
}

const fn before(left: Item, right: Item) -> bool {
    let left_navigation = navigation(left.kind);
    let right_navigation = navigation(right.kind);

    if left_navigation != right_navigation {
        return left_navigation;
    }

    let mut index = 0;

    while index < left.label.len() && index < right.label.len() {
        let left_byte = folded(left.label[index]);
        let right_byte = folded(right.label[index]);

        if left_byte != right_byte {
            return left_byte < right_byte;
        }

        index += 1;
    }

    left.label.len() < right.label.len()
}

/// Stable catalog permutation computed by the constant initializers below.
/// Indices use one byte because every admitted menu has at most 32 items.
/// The complete Item stays in its source catalog; labels, hints, actions,
/// and confirmation kinds are not duplicated into another runtime table.
const fn item_order<const N: usize>(items: &[Item]) -> [u8; N] {
    assert!(items.len() == N && N <= 32);

    let mut order = [0u8; N];
    let mut index = 0;

    while index < N {
        order[index] = index as u8;
        let mut at = index;

        while at > 0 && before(
            items[order[at] as usize],
            items[order[at - 1] as usize],
        ) {
            let previous = order[at - 1];
            order[at - 1] = order[at];
            order[at] = previous;
            at -= 1;
        }

        index += 1;
    }

    order
}

// Each order uses the selected feature-dependent catalog, not its unfiltered
// source table. No boot preparation, mutable cache, or stack sort is needed.
const RADIO_ORDER: [u8; RADIO.len()] = item_order(RADIO);
const CHANNELS_ORDER: [u8; CHANNELS.len()] = item_order(&CHANNELS);
const DISPLAY_ORDER: [u8; DISPLAY.len()] = item_order(DISPLAY);
const STORAGE_ORDER: [u8; STORAGE.len()] = item_order(&STORAGE);
const TOOLS_ORDER: [u8; TOOLS.len()] = item_order(TOOLS);
const AUDIO_ORDER: [u8; AUDIO.len()] = item_order(&AUDIO);
const SCAN_ORDER: [u8; SCAN.len()] = item_order(SCAN);
const VFO_ORDER: [u8; VFO.len()] = item_order(&VFO);
const SYSTEM_ORDER: [u8; SYSTEM.len()] = item_order(&SYSTEM);
const SLOPE_ORDER: [u8; SLOPE.len()] = item_order(&SLOPE);

const fn ordered_indices(group: Group) -> &'static [u8] {
    match group {
        Group::Radio => &RADIO_ORDER,
        Group::Channels => &CHANNELS_ORDER,
        Group::Display => &DISPLAY_ORDER,
        Group::Storage => &STORAGE_ORDER,
        Group::Tools => &TOOLS_ORDER,
        Group::Audio => &AUDIO_ORDER,
        Group::Scan => &SCAN_ORDER,
        Group::Vfo => &VFO_ORDER,
        Group::System => &SYSTEM_ORDER,
        Group::Slope => &SLOPE_ORDER,
    }
}

/// Resolve the displayed row through its immutable catalog permutation.
/// Navigation, numeric jumps, rendering, and compiler fixtures all use this
/// same lookup. The selected row never refers to a separately copied Item.
const fn ordered_item(group: Group, position: usize) -> Item {
    let order = ordered_indices(group);
    assert!(position < order.len());
    group.items()[order[position] as usize]
}

const _: () = {
    // Include the nested VFO page, which is not a root GROUPS entry.
    let groups = [
        Group::Radio, Group::Channels, Group::Display, Group::Storage,
        Group::Tools, Group::Audio, Group::Scan, Group::Vfo, Group::System, Group::Slope,
    ];

    let mut group_index = 0;
    while group_index < groups.len() {
        let group = groups[group_index];
        let items = group.items();
        let order = ordered_indices(group);
        assert!(order.len() == items.len());

        let mut seen = [false; 32];
        let mut position = 0;

        while position < order.len() {
            let original = order[position] as usize;
            assert!(original < items.len());
            assert!(!seen[original]);
            seen[original] = true;

            if position != 0 {
                let previous = order[position - 1] as usize;
                assert!(!before(items[original], items[previous]));

                // Equal sort keys retain their original catalog order.
                if !before(items[previous], items[original]) {
                    assert!(previous < original);
                }
            }

            position += 1;
        }

        position = 0;
        while position < items.len() {
            assert!(seen[position]);
            position += 1;
        }

        group_index += 1;
    }

    // Navigation rank precedes alphabetic order. Case-folded equal labels
    // remain stable even when their underlying actions differ.
    let probe = [
        Item {
            label: b"alpha",
            hint: b"",
            kind: Kind::Setting(Setting::Sql),
        },
        Item {
            label: b"Zulu",
            hint: b"",
            kind: Kind::Action(Action::Help),
        },
        Item {
            label: b"zulu",
            hint: b"",
            kind: Kind::Action(Action::About),
        },
    ];
    let order: [u8; 3] = item_order(&probe);
    assert!(order[0] == 1 && order[1] == 2 && order[2] == 0);

    let empty: [u8; 0] = item_order(&[]);
    assert!(empty.is_empty());
};

const fn digit_position(key: Key) -> Option<usize> {
    match key {
        Key::One => Some(0),
        Key::Two => Some(1),
        Key::Three => Some(2),
        Key::Four => Some(3),
        Key::Five => Some(4),
        Key::Six => Some(5),
        Key::Seven => Some(6),
        Key::Eight => Some(7),
        Key::Nine => Some(8),
        Key::Zero => Some(9),
        _ => None,
    }
}

impl Group {
    const fn title(self) -> &'static [u8] {
        match self {
            Self::Radio => b"RADIO",
            Self::Channels => b"CHANNELS",
            Self::Display => b"DISPLAY",
            Self::Storage => b"STORAGE",
            Self::Tools => b"TOOLS",
            Self::Audio => b"AUDIO",
            Self::Scan => b"SCAN",
            Self::Vfo => b"VFO SETTINGS",
            Self::System => b"SYSTEM",
            Self::Slope => b"SLOPECAST / SC-FM",
        }
    }

    const fn hint(self) -> &'static [u8] {
        match self {
            Self::Radio => b"Receive profile and signal",
            Self::Channels => b"Sixteen channel slots",
            Self::Display => b"Presentation, not RF settings",
            Self::Storage => b"Explicit persistent operations",
            Self::Tools => b"Diagnostics and controls",
            Self::Audio => b"Local cues and transmit tails",
            Self::Scan => b"Timing and channel eligibility",
            Self::Vfo => b"Active VFO profile, not MR",
            Self::System => b"Build identity and key reference",
            Self::Slope => b"Shifted FM, not an AM transmitter",
        }
    }

    const fn items(self) -> &'static [Item] {
        match self {
            Self::Radio => RADIO,
            Self::Channels => &CHANNELS,
            Self::Display => DISPLAY,
            Self::Storage => &STORAGE,
            Self::Tools => TOOLS,
            Self::Audio => &AUDIO,
            Self::Scan => SCAN,
            Self::Vfo => &VFO,
            Self::System => &SYSTEM,
            Self::Slope => &SLOPE,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Radio => 0, Self::Channels => 1, Self::Display => 2,
            Self::Storage => 3, Self::Tools => 4, Self::Audio => 5,
            Self::Scan => 6,
            Self::Vfo => 7,
            Self::System => 8,
            Self::Slope => 9,
        }
    }
}

impl Setting {
    const fn count(self) -> u32 {
        match self {
            Self::Sql => 10,
            Self::SlopeDelta => 500,

            Self::Slot => 16,
            Self::Step | Self::Refresh => 3,
            Self::Roger => 5,
            Self::Power => crate::app::tx_options::POWER_COUNT as u32,
            Self::Tot => crate::app::tx_options::timeout_count(),
            Self::Autosave | Self::ScanResume | Self::SpectrumStep
                | Self::HomeLayout | Self::HomeMeter | Self::BatteryDisplay
                | Self::MenuStyle | Self::TxVisual => 3,
            Self::UiBeep => 4,
            Self::HomeDetail => 6,
            Self::SpectrumRssi => 33,
            Self::SpectrumNoise => 9,
            Self::ScanDwell | Self::ScanHold => 4,
            Self::Permit => 4,
            _ => 2,
        }
    }

    const fn title(self) -> &'static [u8] {
        match self {
            Self::Rx => b"RECEIVE",
            Self::Vfo => b"VFO",
            Self::Mode => b"TUNING MODE",
            Self::Band => b"RECEIVE PATH",
            Self::Width => b"CHANNEL WIDTH",
            Self::Demod => b"RX MODULATION",
            Self::Step => b"TUNING STEP",
            Self::Sql => b"SQUELCH",
            Self::Monitor => b"MONITOR",
            Self::Light => b"BACKLIGHT",
            Self::Refresh => b"SCREEN REFRESH",
            Self::Slot => b"CHANNEL SLOT",
            Self::Roger => b"ROGER TAIL",
            Self::Permit => b"TALK PERMIT",
            Self::BootTone => b"BOOT TONE",
            Self::Power => b"TRANSMIT POWER",
            Self::Ham => b"HAM MODE",
            Self::Tot => b"TRANSMIT TIMEOUT",
            Self::DualWatch => b"DUAL WATCH",
            Self::SlopeEnabled => b"SLOPECAST ENABLE",
            Self::SlopeSide => b"SC-FM TX SIDE",
            Self::SlopeDelta => b"SC-FM SHIFT KHZ",
            Self::SlopeWidth => b"SC-FM TX WIDTH",


            Self::Autosave => b"AUTOSAVE",
            Self::ScanDwell => b"SCAN DWELL",
            Self::ScanHold => b"SCAN HOLD",
            Self::ScanResume => b"SCAN RESUME",
            Self::ScanDirection => b"SCAN DIRECTION",
            Self::ScanSkip => b"SKIP SELECTED SLOT",
            Self::SpectrumEnabled => b"SPECTRUM",
            Self::SpectrumStep => b"SPECTRUM STEP",
            Self::SpectrumRssi => b"SPECTRUM RSSI",
            Self::SpectrumNoise => b"SPECTRUM NOISE",
            Self::HomeLayout => b"HOME LAYOUT",
            Self::HomePrimary => b"PRIMARY DISPLAY",
            Self::HomeDetail => b"HOME INFORMATION",
            Self::HomeMeter => b"RECEIVE METER",
            Self::BatteryDisplay => b"BATTERY DISPLAY",
            Self::MenuStyle => b"MENU STYLE",
            Self::UiBeep => b"UI FEEDBACK",
            Self::TxVisual => b"TX VISUALIZATION",
            Self::UiHints => b"OPTIONAL HINTS",
        }
    }

    const fn text(self, value: u32) -> &'static [u8] {
        match self {
            Self::Rx | Self::Monitor | Self::Light | Self::BootTone
                | Self::Ham | Self::DualWatch | Self::SlopeEnabled =>
            {
                if value == 0 { b"OFF" } else { b"ON" }
            }
            Self::SlopeSide => if value == 0 { b"LOWER" } else { b"UPPER" },
            Self::SlopeWidth => if value == 0 { b"WIDE" } else { b"NARROW" },
            Self::SlopeDelta => b"",
            Self::Vfo => if value == 0 { b"A" } else { b"B" },
            Self::Mode => if value == 0 { b"VFO" } else { b"MR" },
            Self::Band => if value == 0 { b"LOW" } else { b"HIGH" },
            Self::Width => if value == 0 { b"WIDE" } else { b"NARROW" },
            Self::Demod => if value == 0 { b"FM" } else { b"AM" },
            Self::Step => match value { 0 => b"12.5K", 1 => b"25K", _ => b"100K" },
            Self::Refresh => match value { 0 => b"125MS", 1 => b"250MS", _ => b"500MS" },
            Self::Sql | Self::Slot => b"",
            Self::Power => crate::app::tx_policy::power_name(value as u8),
            Self::Tot => crate::app::tx_options::timeout_name(value),
            Self::Autosave => match value {
                0 => b"NO AUTO",
                1 => b"WARN",
                _ => b"AUTO",
            },
            Self::ScanDwell => match value {
                0 => b"200MS",
                1 => b"300MS",
                2 => b"500MS",
                _ => b"1000MS",
            },
            Self::ScanHold => match value {
                0 => b"300MS",
                1 => b"500MS",
                2 => b"1000MS",
                _ => b"2000MS",
            },
            Self::ScanResume => match value {
                0 => b"CARRIER",
                1 => b"TIMED",
                _ => b"STOP",
            },
            Self::ScanDirection => if value == 0 { b"UP" } else { b"DOWN" },
            Self::ScanSkip => if value == 0 { b"INCLUDE" } else { b"SKIP" },
            Self::SpectrumEnabled => if value == 0 { b"OFF" } else { b"ON" },
            Self::SpectrumStep => match value {
                0 => b"12.5K",
                1 => b"25K",
                _ => b"100K",
            },
            Self::SpectrumRssi | Self::SpectrumNoise => b"",
            Self::HomeLayout => match value {
                0 => b"CLEAN", 1 => b"PANEL", _ => b"DETAIL",
            },
            Self::HomePrimary => if value == 0 { b"FREQ" } else { b"NAME" },
            Self::HomeDetail => match value {
                0 => b"PROFILE",
                1 => b"SIGNAL",
                2 => b"BATTERY",
                3 => b"OTHER",
                4 => b"UPTIME",
                _ => b"OFF",
            },
            Self::HomeMeter => match value {
                0 => b"OFF", 1 => b"BAR", _ => b"SEG",
            },
            Self::BatteryDisplay => match value {
                0 => b"ICON", 1 => b"VOLTS", _ => b"OFF",
            },
            Self::MenuStyle => match value {
                0 => b"CURSOR", 1 => b"INVERSE", _ => b"OUTLINE",
            },
            Self::UiBeep => match value {
                0 => b"OFF", 1 => b"KEYS", 2 => b"ACTIONS", _ => b"ALL",
            },
            Self::TxVisual => match value {
                0 => b"OFF", 1 => b"LEVEL", _ => b"HISTORY",
            },
            Self::UiHints => if value == 0 { b"OFF" } else { b"ON" },
            Self::Roger => match value {
                0 => b"OFF", 1 => b"SINGLE", 2 => b"PAIR",
                3 => b"TRIPLE", _ => b"MDC",
            },
            Self::Permit => match value {
                0 => b"OFF", 1 => b"TICK", 2 => b"RISE", _ => b"PAIR",
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Values {
    pub(super) rx: bool,
    pub(super) dual_watch: bool,
    pub(super) vfo: usize,
    pub(super) channel: bool,
    pub(super) populated: bool,
    pub(super) slot_populated: bool,
    pub(super) uhf: bool,
    pub(super) narrow: bool,
    pub(super) am: bool,
    pub(super) step: u32,
    pub(super) sql: u8,
    pub(super) monitor: bool,
    pub(super) light: bool,
    pub(super) period: u32,
    pub(super) slot: usize,
    pub(super) sounds: crate::app::sound::Settings,
    pub(super) slope: crate::slope::Settings,
    pub(super) slope_frequency: Result<u32, u32>,
    pub(super) ham: bool,
    pub(super) power: u8,
    pub(super) tot: u8,
    pub(super) tx_signal: u16,
    pub(super) rx_signal: u16,
    pub(super) dirty: bool,
    pub(super) preferences: crate::app::preferences::Preferences,
    pub(super) appearance: crate::app::appearance::Settings,
}

impl Values {
    const fn value(self, setting: Setting) -> u32 {
        match setting {
            Setting::Rx => self.rx as u32,
            Setting::DualWatch => self.dual_watch as u32,
            Setting::SlopeEnabled => self.slope.enabled as u32,
            Setting::SlopeSide => self.slope.upper as u32,
            Setting::SlopeDelta => self.slope.delta as u32 - 1,
            Setting::SlopeWidth => self.slope.narrow as u32,
            Setting::Vfo => self.vfo as u32,
            Setting::Mode => self.channel as u32,
            Setting::Band => self.uhf as u32,
            Setting::Width => self.narrow as u32,
            Setting::Demod => self.am as u32,
            Setting::Step => match self.step { 12_500 => 0, 25_000 => 1, _ => 2 },
            Setting::Sql => self.sql as u32,
            Setting::Monitor => self.monitor as u32,
            Setting::Light => self.light as u32,
            Setting::Refresh => match self.period { 125 => 0, 250 => 1, _ => 2 },
            Setting::Slot => self.slot as u32,
            Setting::Roger => self.sounds.roger as u32,
            Setting::Permit => self.sounds.permit as u32,
            Setting::BootTone => self.sounds.boot as u32,
            Setting::Ham => self.ham as u32,
            Setting::Power => self.power as u32,
            Setting::Tot => self.tot as u32,


            Setting::HomeLayout => self.appearance.layout as u32,
            Setting::HomePrimary => self.appearance.primary as u32,
            Setting::HomeDetail => self.appearance.detail as u32,
            Setting::HomeMeter => self.appearance.meter as u32,
            Setting::BatteryDisplay => self.appearance.battery as u32,
            Setting::MenuStyle => self.appearance.menu as u32,
            Setting::UiBeep => self.appearance.beeps as u32,
            Setting::TxVisual => self.appearance.tx_visual as u32,
            Setting::UiHints => self.appearance.hints as u32,
            Setting::Autosave => self.preferences.autosave as u32,
            Setting::ScanDwell => self.preferences.dwell as u32,
            Setting::ScanHold => self.preferences.hold as u32,
            Setting::ScanResume => self.preferences.resume as u32,
            Setting::ScanDirection => self.preferences.reverse as u32,
            Setting::ScanSkip => self.preferences.skipped(self.slot) as u32,
            Setting::SpectrumEnabled => self.preferences.spectrum as u32,
            Setting::SpectrumStep => self.preferences.spectrum_step as u32,
            Setting::SpectrumRssi => {
                if self.preferences.spectrum_rssi == 511 {
                    32
                } else {
                    self.preferences.spectrum_rssi as u32 / 16
                }
            }
            Setting::SpectrumNoise => {
                if self.preferences.spectrum_noise == 127 {
                    8
                } else {
                    self.preferences.spectrum_noise as u32 / 16
                }
            }
        }
    }

    const fn editable(self, setting: Setting) -> bool {
        match setting {
            Setting::Band | Setting::Width | Setting::Demod | Setting::Step
                | Setting::DualWatch => !self.channel,
            Setting::Rx => self.populated,
            _ => true,
        }
    }
}

#[derive(Clone, Copy)]
struct Editor {
    setting: Setting,
    value: u32,
    increment: u32,
}

pub(super) enum Effect {
    Home,
    Set(Setting, u32),
    Action(Action),
    File(FileAction),
    Test(crate::debug::Test),
}

pub(super) struct Menu {
    root: usize,
    group: Option<Group>,
    cursors: [usize; 10],
    editor: Option<Editor>,
    confirmation: Option<Confirmation>,
    note: Option<&'static [u8]>,
}

impl Menu {
    pub(super) const fn new() -> Self {
        Self {
            root: 0,
            group: None,
            cursors: [0; 10],
            editor: None,
            confirmation: None,
            note: None,
        }
    }

    pub(super) const fn open(&mut self) {
        self.group = None;
        self.editor = None;
        self.confirmation = None;
        self.note = None;
    }

    const fn selected(&self) -> usize {
        match self.group {
            Some(group) => self.cursors[group.index()],
            None => self.root,
        }
    }

    const fn count(&self) -> usize {
        match self.group {
            Some(group) => group.items().len(),
            None => GROUPS.len(),
        }
    }

    pub(super) const fn key(&mut self, event: Event, values: Values) -> Option<Effect> {
        if !matches!(event.edge, Edge::Down | Edge::Repeat) {
            return None;
        }
        let down = matches!(event.edge, Edge::Down);

        if down && self.editor.is_none() && self.confirmation.is_none() {
            if let Some(position) = digit_position(event.key) {
                if position < self.count() {
                    match self.group {
                        Some(group) => self.cursors[group.index()] = position,
                        None => self.root = position,
                    }

                    self.note = None;
                }

                return None;
            }
        }

        if matches!(event.key, Key::Exit) && down {
            self.note = None;
            if self.confirmation.is_some() {
                self.confirmation = None;
            } else if self.editor.is_some() {
                self.editor = None;
            } else if let Some(group) = self.group {
                self.group = if matches!(group, Group::Vfo | Group::Slope) {
                    Some(Group::Radio)
                } else {
                    None
                };
            } else {
                return Some(Effect::Home);
            }
            return None;
        }

        if down && matches!(event.key, Key::F) {
            if let Some(mut editor) = self.editor {
                if matches!(editor.setting, Setting::SlopeDelta) {
                    editor.increment = if editor.increment == 10 { 1 } else { 10 };
                    self.editor = Some(editor);
                }
            }
            return None;
        }

        if matches!(event.key, Key::Up | Key::Down) {
            self.note = None;
            let up = matches!(event.key, Key::Up);

            if let Some(mut editor) = self.editor {
                let count = editor.setting.count();
                editor.value = if up {
                    (editor.value + editor.increment) % count
                } else {
                    (editor.value + count - editor.increment) % count
                };
                self.editor = Some(editor);
            } else if self.confirmation.is_none() {
                let count = self.count();
                let selected = self.selected();
                let next = if up {
                    (selected + count - 1) % count
                } else {
                    (selected + 1) % count
                };
                match self.group {
                    Some(group) => self.cursors[group.index()] = next,
                    None => self.root = next,
                }
            }
            return None;
        }

        if !matches!(event.key, Key::Menu) || !down {
            return None;
        }
        self.note = None;

        if let Some(action) = self.confirmation {
            self.confirmation = None;
            return Some(match action {
                Confirmation::Local(action) => Effect::Action(action),
                Confirmation::File(action) => Effect::File(action),
                Confirmation::Test(test) => Effect::Test(test),
            });
        }
        if let Some(editor) = self.editor {
            self.editor = None;
            return Some(Effect::Set(editor.setting, editor.value));
        }

        let group = match self.group {
            Some(group) => group,
            None => {
                self.group = Some(GROUPS[self.root]);
                return None;
            }
        };
        let item = ordered_item(group, self.selected());

        match item.kind {
            Kind::Page(Group::Slope) if !cfg!(feature = "fam") => {
                self.note = Some(b"SLOPECAST NOT IN THIS IMAGE");
            }
            Kind::Page(Group::Vfo) if values.channel => {
                self.note = Some(b"SELECT VFO MODE FIRST");
            }
            Kind::Page(group) => self.group = Some(group),
            Kind::Setting(setting) => {
                if values.editable(setting) {
                    self.editor = Some(Editor {
                        setting,
                        value: values.value(setting),
                        increment: if matches!(setting, Setting::SlopeDelta) { 10 } else { 1 },
                    });
                } else {
                    self.note = Some(if matches!(setting, Setting::Rx) {
                        b"SELECT A POPULATED CHANNEL"
                    } else {
                        b"SELECT VFO MODE FIRST"
                    });
                }
            }
            Kind::Action(Action::TxRoute | Action::TxSignal | Action::RxSignal) if values.channel => {
                self.note = Some(b"SELECT VFO MODE FIRST");
            }
            Kind::Local(Action::SaveChannel) if values.channel => {
                self.note = Some(b"SELECT VFO TO STORE A CHANNEL");
            }
            Kind::Action(Action::EditChannel) | Kind::Local(Action::DeleteChannel)
                if !values.slot_populated =>
            {
                self.note = Some(b"SELECT A POPULATED CHANNEL");
            }
            Kind::Action(action) => return Some(Effect::Action(action)),
            Kind::Local(action) => {
                self.confirmation = Some(Confirmation::Local(action));
            }
            Kind::File(action) => self.confirmation = Some(Confirmation::File(action)),
            Kind::Test(test) => self.confirmation = Some(Confirmation::Test(test)),
        }
        None
    }

    fn draw_value(frame: &mut Frame, x: i16, y: i16, setting: Setting, value: u32, size: Size) {
        match setting {

            Setting::SlopeDelta => {
                frame.text_size(x, y, &crate::slope::khz((value + 1) as u16), size);
            }
            Setting::Sql => widgets::number(frame, x, y, value, size),
            Setting::SpectrumRssi => {
                widgets::number(frame, x, y, if value == 32 { 511 } else { value * 16 }, size);
            }
            Setting::SpectrumNoise => {
                widgets::number(frame, x, y, if value == 8 { 127 } else { value * 16 }, size);
            }
            Setting::Slot => widgets::number(frame, x, y, value + 1, size),
            _ => frame.text_size(x, y, setting.text(value), size),
        }
    }

    pub(super) fn render(&self, frame: &mut Frame, values: Values) {
        frame.clear();

        if let Some(action) = self.confirmation {
            frame.text(2, 0, b"CONFIRM");
            frame.fill(0, 8, 128, 1, true);
            let title: &[u8] = match action {
                Confirmation::Local(Action::SaveChannel) => b"STORE",
                Confirmation::Local(Action::DeleteChannel) => b"DELETE",
                Confirmation::Local(_) => b"CHANGE",
                Confirmation::File(FileAction::Save) => b"SAVE",
                Confirmation::File(FileAction::Load) => b"LOAD",
                Confirmation::File(FileAction::ResetRam) => b"RESET",
                Confirmation::File(FileAction::TestTorn) => b"TORN",
                Confirmation::Test(crate::debug::Test::Fault) => b"FAULT",
                Confirmation::Test(crate::debug::Test::Watchdog) => b"IWDG",
                Confirmation::Test(crate::debug::Test::Reentry) => b"RAM",
            };
            frame.text_size(10, 18, title, Size::Large);
            let hint: &[u8] = match action {
                Confirmation::Local(Action::SaveChannel) => b"Replace selected slot in RAM",
                Confirmation::Local(Action::DeleteChannel) => b"Clear selected slot in RAM",
                Confirmation::Local(_) => b"Apply a RAM-only change",
                Confirmation::File(FileAction::Save) => b"Commit settings and channels",
                Confirmation::File(FileAction::Load) if values.dirty => {
                    b"UNSAVED RAM CHANGES WILL BE LOST"
                }
                Confirmation::File(FileAction::ResetRam) if values.dirty => {
                    b"UNSAVED RAM CHANGES WILL BE LOST"
                }
                Confirmation::File(FileAction::Load) => b"Restore saved radio settings",
                Confirmation::File(FileAction::ResetRam) => b"RAM only. Saved copy remains",
                Confirmation::File(FileAction::TestTorn) => b"Test body without commit",
                Confirmation::Test(crate::debug::Test::Fault) => b"Exception. Unsaved RAM is lost",
                Confirmation::Test(crate::debug::Test::Watchdog) => b"Stop IWDG service. RAM is lost",
                Confirmation::Test(crate::debug::Test::Reentry) => b"App restart. Unsaved RAM is lost",
            };
            frame.text_size(2, 42, hint, Size::Small);
            if matches!(action, Confirmation::Local(_)) {
                frame.text_size(2, 49, b"SLOT", Size::Small);
                widgets::number(frame, 26, 49, values.slot as u32 + 1, Size::Small);
                frame.text_size(48, 49, b"FLASH UNCHANGED", Size::Small);
            }
            frame.text_size(2, 56, b"MENU CONFIRM / EXIT CANCEL", Size::Small);
            return;
        }

        if let Some(editor) = self.editor {
            frame.text(2, 0, editor.setting.title());
            frame.fill(0, 8, 128, 1, true);
            Self::draw_value(frame, 12, 20, editor.setting, editor.value, Size::Large);
            frame.text_size(2, 44, if matches!(editor.setting, Setting::SlopeDelta) {
                if editor.increment == 10 {
                    b"STEP 1.0 KHZ / F: 0.1 KHZ"
                } else {
                    b"STEP 0.1 KHZ / F: 1.0 KHZ"
                }
            } else {
                b"UP INCREASE / DOWN DECREASE"
            }, Size::Small);
            frame.text_size(2, 56, b"MENU APPLY / EXIT CANCEL", Size::Small);
            return;
        }

        let title = self.group.map_or(b"MENU" as &[u8], Group::title);
        frame.text(2, 0, title);

        if values.dirty {
            frame.text_size(88, 1, b"*", Size::Small);
        }

        widgets::number(frame, 101, 1, self.selected() as u32 + 1, Size::Small);
        frame.text_size(109, 1, b"/", Size::Small);
        widgets::number(frame, 113, 1, self.count() as u32, Size::Small);
        frame.fill(0, 8, 128, 1, true);

        let selected = self.selected();
        let first = selected.saturating_sub(3);
        for row in 0..4 {
            let index = first + row;
            if index >= self.count() { break; }
            let y = 11 + row as i16 * 10;

            match self.group {
                None => {
                    frame.text_size(17, y + 1, GROUPS[index].title(), Size::Small);
                    frame.text(112, y, b">");
                }
                Some(group) => {
                    let item = ordered_item(group, index);
                    frame.text_size(17, y + 1, item.label, Size::Small);
                    match item.kind {
                        Kind::Setting(setting) => {
                            Self::draw_value(frame, 90, y + 1, setting, values.value(setting), Size::Small);
                        }
                        Kind::Action(Action::RxSignal) => {
                            frame.text_size(
                                90, y + 1, &crate::tx_signal::text(values.rx_signal),
                                Size::Small,
                            );
                        }
                        Kind::Action(Action::TxSignal) => {
                            frame.text_size(
                                90, y + 1, &crate::tx_signal::text(values.tx_signal),
                                Size::Small,
                            );
                        }
                        Kind::Page(_) | Kind::Action(_) | Kind::Local(_)
                            | Kind::File(_) | Kind::Test(_) =>
                        {
                            frame.text(112, y, b">");
                        }
                    }
                }
            }
            widgets::number(frame, 2, y + 1, index as u32 + 1, Size::Small);

            if index == selected {
                match values.appearance.menu {
                    0 => {
                        frame.fill(0, y - 1, 1, 9, true);
                        frame.fill(122, y - 1, 1, 9, true);
                    }
                    1 => frame.invert(0, y - 1, 123, 9),
                    _ => frame.outline(0, y - 1, 123, 9, true),
                }
            }
        }

        widgets::scrollbar(frame, self.count(), first, 4);
        let hint = self.note.unwrap_or(match self.group {
            Some(group) => ordered_item(group, selected).hint,
            None => GROUPS[selected].hint(),
        });
        if matches!(self.group, Some(Group::Slope)) && self.note.is_none() {
            if !values.am {
                frame.text_size(2, 52, b"SELECT AM TO USE SC-FM", Size::Small);
            } else {
                frame.text_size(2, 52, b"RF TX", Size::Small);
                match values.slope_frequency {
                    Ok(frequency) => widgets::frequency(
                        frame, 30, 52, frequency, Size::Small,
                    ),
                    Err(0x700C | 0x700F) => frame.text_size(
                        30, 52, b"DISABLED", Size::Small,
                    ),
                    Err(_) => frame.text_size(
                        30, 52, b"ROUTE OR RANGE REFUSED", Size::Small,
                    ),
                }
            }
        } else if values.appearance.hints || self.note.is_some() {
            frame.text_size(2, 52, hint, Size::Small);
        }

        frame.text_size(2, 58, b"MENU SELECT / EXIT BACK", Size::Small);
    }
}

const fn press(key: Key) -> Event {
    Event { key, edge: Edge::Down }
}

const fn find_label(group: Group, label: &[u8]) -> usize {
    let mut index = 0;

    while index < group.items().len() {
        let item = ordered_item(group, index);
        let mut same = item.label.len() == label.len();
        let mut byte = 0;

        while same && byte < label.len() {
            same = item.label[byte] == label[byte];
            byte += 1;
        }

        if same {
            return index;
        }

        index += 1;
    }

    panic!("menu label missing")
}

const _: () = {
    let values = Values {
        rx: false,
        dual_watch: false,
        vfo: 0,
        channel: false,
        populated: true,
        slot_populated: true,
        uhf: false,
        narrow: false,
        am: false,
        step: 12500,
        sql: 3,
        monitor: false,
        light: true,
        period: 250,
        slot: 0,
        sounds: crate::app::sound::Settings::new(),
        slope: crate::slope::Settings::new(),
        slope_frequency: Err(0x700F),
        ham: true,
        power: 0,
        tot: crate::app::tx_options::DEFAULT_TOT,
        tx_signal: 0,
        rx_signal: 0,
        dirty: false,
        preferences: crate::app::preferences::Preferences::new(),
        appearance: crate::app::appearance::Settings::new(),
    };

    let mut group_index = 0;

    while group_index < GROUPS.len() {
        let group = GROUPS[group_index];
        let mut index = 1;

        while index < group.items().len() {
            let previous = ordered_item(group, index - 1);
            let current = ordered_item(group, index);
            assert!(!before(current, previous));
            index += 1;
        }

        group_index += 1;
    }

    let mut editor = Menu::new();
    editor.group = Some(Group::Radio);
    editor.cursors[Group::Radio.index()] = find_label(Group::Radio, b"Receive");

    assert!(editor.key(press(Key::Menu), values).is_none());
    assert!(editor.editor.is_some());
    assert!(editor.key(press(Key::Up), values).is_none());

    assert!(matches!(
        editor.key(press(Key::Menu), values),
        Some(Effect::Set(Setting::Rx, 1)),
    ));

    let mut storage = Menu::new();
    storage.group = Some(Group::Storage);
    storage.cursors[Group::Storage.index()] = find_label(Group::Storage, b"Save");

    assert!(storage.key(press(Key::Menu), values).is_none());
    assert!(storage.confirmation.is_some());

    let repeated = Event { key: Key::Menu, edge: Edge::Repeat };
    assert!(storage.key(repeated, values).is_none());
    assert!(storage.confirmation.is_some());

    assert!(matches!(
        storage.key(press(Key::Menu), values),
        Some(Effect::File(FileAction::Save)),
    ));

    if cfg!(feature = "debug-tools") {
        let mut fault = Menu::new();
        fault.group = Some(Group::Tools);
        fault.cursors[Group::Tools.index()] = find_label(Group::Tools, b"Test fault");
    
        assert!(fault.key(press(Key::Menu), values).is_none());
        assert!(fault.confirmation.is_some());
        assert!(fault.key(repeated, values).is_none());
        assert!(fault.key(press(Key::Exit), values).is_none());
        assert!(fault.confirmation.is_none());
    
        assert!(fault.key(press(Key::Menu), values).is_none());
        assert!(matches!(
            fault.key(press(Key::Menu), values),
            Some(Effect::Test(crate::debug::Test::Fault)),
        ));
    }

    let mut replace = Menu::new();
    replace.group = Some(Group::Channels);
    replace.cursors[Group::Channels.index()] = find_label(Group::Channels, b"Store VFO");

    assert!(replace.key(press(Key::Menu), values).is_none());
    assert!(replace.confirmation.is_some());
    assert!(replace.key(repeated, values).is_none());

    assert!(matches!(
        replace.key(press(Key::Menu), values),
        Some(Effect::Action(Action::SaveChannel)),
    ));

    let mut empty = values;
    empty.slot_populated = false;

    let mut deletion = Menu::new();
    deletion.group = Some(Group::Channels);
    deletion.cursors[Group::Channels.index()] = find_label(Group::Channels, b"Delete");

    assert!(deletion.key(press(Key::Menu), empty).is_none());
    assert!(deletion.confirmation.is_none());
    assert!(deletion.note.is_some());

    let mut channel = values;
    channel.channel = true;

    let mut locked = Menu::new();
    locked.group = Some(Group::Vfo);
    locked.cursors[Group::Vfo.index()] = find_label(Group::Vfo, b"Band");

    assert!(locked.key(press(Key::Menu), channel).is_none());
    assert!(locked.editor.is_none());
    assert!(locked.note.is_some());

    let mut jumping = Menu::new();
    assert!(jumping.key(press(Key::Five), values).is_none());
    assert!(jumping.root == 4);
    assert!(jumping.group.is_none());

    jumping.group = Some(Group::Tools);
    assert!(jumping.key(press(Key::Zero), values).is_none());
    let expected = if cfg!(feature = "debug-tools") { 9 } else { 0 };
    assert!(jumping.selected() == expected);

    assert!(jumping.key(press(Key::Ptt), values).is_none());
    assert!(jumping.selected() == expected);

    assert!(DISPLAY.len() == if cfg!(feature = "debug-tools") { 11 } else { 10 });
    let tool_count = if cfg!(feature = "debug-tools") {
        if cfg!(feature = "uart") { 13 } else { 12 }
    } else if cfg!(feature = "diagnostics") {
        if cfg!(feature = "uart") { 4 } else { 3 }
    } else {
        0
    };
    assert!(TOOLS.len() == tool_count);



    assert!(Setting::Autosave.count() == 3);
    assert!(Setting::ScanDwell.count() == 4);
    assert!(Setting::ScanHold.count() == 4);
    assert!(Setting::ScanResume.count() == 3);

    assert!(GROUPS.len() == if cfg!(feature = "diagnostics") { 8 } else { 7 });

    let mut tools_pages = 0;
    let mut system_pages = 0;
    let mut group_at = 0;
    while group_at < GROUPS.len() {
        if matches!(GROUPS[group_at], Group::Tools) {
            tools_pages += 1;
            assert!(!TOOLS.is_empty());
        }
        if matches!(GROUPS[group_at], Group::System) {
            system_pages += 1;
        }
        group_at += 1;
    }
    assert!(system_pages == 1);
    assert!(tools_pages == if cfg!(feature = "diagnostics") { 1 } else { 0 });

    assert!(SYSTEM.len() == 2);
    assert!(matches!(
        ordered_item(Group::System, find_label(Group::System, b"About")).kind,
        Kind::Action(Action::About),
    ));
    assert!(matches!(
        ordered_item(Group::System, find_label(Group::System, b"Controls")).kind,
        Kind::Action(Action::Help),
    ));

    let mut tool_at = 0;
    while tool_at < TOOLS.len() {
        assert!(!matches!(
            TOOLS[tool_at].kind,
            Kind::Action(Action::Help | Action::About),
        ));
        if !cfg!(feature = "debug-tools") {
            assert!(matches!(
                TOOLS[tool_at].kind,
                Kind::Action(Action::Stacks | Action::Power | Action::LastFault | Action::Serial),
            ));
        }
        tool_at += 1;
    }

    if cfg!(feature = "diagnostics") {
        assert!(matches!(
            ordered_item(Group::Tools, find_label(Group::Tools, b"Stack")).kind,
            Kind::Action(Action::Stacks),
        ));
        assert!(matches!(
            ordered_item(Group::Tools, find_label(Group::Tools, b"Power")).kind,
            Kind::Action(Action::Power),
        ));
        assert!(matches!(
            ordered_item(Group::Tools, find_label(Group::Tools, b"Last fault")).kind,
            Kind::Action(Action::LastFault),
        ));
        if cfg!(feature = "uart") {
            assert!(matches!(
                ordered_item(Group::Tools, find_label(Group::Tools, b"UART")).kind,
                Kind::Action(Action::Serial),
            ));
        }
    }
    assert!(RADIO.len() == if cfg!(feature = "fam") { 10 } else { 9 });
    assert!(VFO.len() == 9 && CHANNELS.len() == 7);


    let mut nested = Menu::new();
    nested.group = Some(Group::Radio);
    nested.cursors[Group::Radio.index()] = find_label(Group::Radio, b"VFO settings");
    assert!(nested.key(press(Key::Menu), channel).is_none());
    assert!(matches!(nested.group, Some(Group::Radio)));
    assert!(nested.note.is_some());
    assert!(nested.key(press(Key::Menu), values).is_none());
    assert!(matches!(nested.group, Some(Group::Vfo)));
    assert!(nested.key(press(Key::Exit), values).is_none());
    assert!(matches!(nested.group, Some(Group::Radio)));

    nested.group = Some(Group::Vfo);
    nested.cursors[Group::Vfo.index()] = find_label(Group::Vfo, b"TX signal");
    assert!(nested.key(press(Key::Menu), channel).is_none());
    assert!(nested.note.is_some());
    assert!(matches!(
        nested.key(press(Key::Menu), values),
        Some(Effect::Action(Action::TxSignal)),
    ));

    nested.cursors[Group::Vfo.index()] = find_label(Group::Vfo, b"RX signal");
    assert!(nested.key(press(Key::Menu), channel).is_none());
    assert!(nested.note.is_some());
    assert!(matches!(
        nested.key(press(Key::Menu), values),
        Some(Effect::Action(Action::RxSignal)),
    ));
    assert!(Setting::Power.count() == 14);
    assert!(Setting::DualWatch.count() == 2);
    assert!(values.editable(Setting::DualWatch));
    assert!(!channel.editable(Setting::DualWatch));

    let mut dual_menu = Menu::new();
    dual_menu.group = Some(Group::Radio);
    dual_menu.cursors[Group::Radio.index()] = find_label(Group::Radio, b"Dual Watch");
    assert!(dual_menu.key(press(Key::Menu), values).is_none());
    assert!(dual_menu.key(press(Key::Up), values).is_none());
    assert!(matches!(
        dual_menu.key(press(Key::Menu), values),
        Some(Effect::Set(Setting::DualWatch, 1)),
    ));
    assert!(Setting::Tot.count() == 6);
    assert!(SCAN.len() == if cfg!(feature = "spectrum") { 12 } else { 7 });

    let mut scope_entries = 0;
    let mut scan_index = 0;
    while scan_index < SCAN.len() {
        if matches!(
            SCAN[scan_index].kind,
            Kind::Action(Action::Spectrum)
                | Kind::Setting(
                    Setting::SpectrumEnabled | Setting::SpectrumStep
                        | Setting::SpectrumRssi | Setting::SpectrumNoise
                )
        ) {
            scope_entries += 1;
        }
        scan_index += 1;
    }
    assert!(scope_entries == if cfg!(feature = "spectrum") { 5 } else { 0 });

    assert!(matches!(
        ordered_item(Group::Scan, find_label(Group::Scan, b"Range")).kind,
        Kind::Action(Action::ScanRange),
    ));
    assert!(matches!(
        ordered_item(Group::Scan, find_label(Group::Scan, b"Start / stop")).kind,
        Kind::Action(Action::Scan),
    ));
    assert!(Setting::SpectrumRssi.count() == 33);
    assert!(Setting::SpectrumNoise.count() == 9);
    assert!(STORAGE.len() == 6);

};
const _: () = {
    let mut entries = 0;
    let mut index = 0;
    while index < RADIO.len() {
        if matches!(RADIO[index].kind, Kind::Page(Group::Slope)) {
            entries += 1;
        }
        index += 1;
    }
    assert!(entries == if cfg!(feature = "fam") { 1 } else { 0 });
    assert!(Setting::SlopeDelta.count() == 500);
    assert!(Setting::SlopeWidth.count() == 2);
    assert!(SLOPE.len() == 4);
};
