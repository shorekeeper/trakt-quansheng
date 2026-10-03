//! Receive threshold selection: factory FM rows and a separate AM RSSI scale.
//!
//! FM uses all six factory columns together. AM does not project only their
//! RSSI pair into a standalone detector. It selects a fixed software scale:
//! levels 1..9 open at 160 + 16 * (level - 1) raw counts and remain open at
//! eight counts below that value. Level zero retains the bypass profile.
//!
//! The AM scale is an explicit operating policy, not a calibrated noise floor,
//! dBm conversion, sensitivity specification, or proof of channel vacancy.
//! It applies to both AM widths and all admitted RX intervals. Signals below
//! the selected threshold remain muted. No running noise estimator can learn
//! an active carrier as the background and then silently admit transmission.
//!
//! Noise and glitch conditions are disabled for AM by their indicator maxima.
//! Carrier timing, stale-history handling, and monitor/PTT behavior belong to
//! app::squelch::Gate and are not changed by threshold selection.
//! adjusted describes normalization of factory FM data only; it is false
//! for the fixed AM scale. Memory acquisition remains a separate prerequisite.
//!
//! UHF starts at block offset 000; VHF at 060. Each column spans 16 bytes,
//! and the selected SQL level indexes one byte within every column.
//! Column order is RSSI open/close, noise open/close, glitch close/open.
//!
//! Noise is limited to the indicator's seven-bit range. Close thresholds
//! are ordered around open thresholds so an overlapping factory pair cannot
//! require a better signal to remain open than it required to open.
//! The adjusted flag records that normalization; raw calibration is unchanged.
//!
//! An all-zero or all-FF selected row is rejected for levels one through nine.
//! Level zero supplies a bypass profile. None of these checks authenticates
//! the calibration or replaces the prior two-read comparison.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Thresholds {
    pub(crate) open_rssi: u32,
    pub(crate) close_rssi: u32,
    pub(crate) open_noise: u32,
    pub(crate) close_noise: u32,
    pub(crate) open_glitch: u32,
    pub(crate) close_glitch: u32,
    pub(crate) adjusted: bool,
}

impl Thresholds {
    /// Independent AM threshold policy in raw nine-bit RSSI counts.
    /// No factory byte is used as a standalone AM opening threshold.
    /// Validate the level before subtraction so malformed input cannot
    /// underflow or index a table. Level zero does not imply a busy carrier;
    /// the caller's normal bypass handling determines speaker admission.
    pub(crate) const fn am(level: u8) -> Result<Self, u32> {
        if level > 9 {
            return Err(0x5102);
        }
        if level == 0 {
            return Ok(Self::disabled());
        }

        let open = 160 + (level as u32 - 1) * 16;
        Ok(Self {
            open_rssi: open,
            close_rssi: open - 8,
            open_noise: 127,
            close_noise: 127,
            open_glitch: 255,
            close_glitch: 255,
            adjusted: false,
        })
    }

    pub(crate) const fn disabled() -> Self {
        Self {
            open_rssi: 0,
            close_rssi: 0,
            open_noise: 127,
            close_noise: 127,
            open_glitch: 255,
            close_glitch: 255,
            adjusted: false,
        }
    }
}

const fn minimum(a: u32, b: u32) -> u32 {
    if a < b { a } else { b }
}

const fn maximum(a: u32, b: u32) -> u32 {
    if a > b { a } else { b }
}

/// Select one modulation's threshold source after RX frequency admission.
/// FM retains the established calibration-table boundary at 174 MHz.
/// AM reads no threshold row from block, including for extended RX.
/// Memory supplies this call only after acquisition has completed.
pub(crate) const fn select(
    block: &[u8; 512],
    frequency: u32,
    level: u8,
    am: bool,
) -> Result<Thresholds, u32> {
    if crate::profile::Band::receive(frequency).is_none() {
        return Err(0x5101);
    }

    if am {
        Thresholds::am(level)
    } else {
        decode(block, frequency <= 174_000_000, level)
    }
}

pub(crate) const fn decode(
    block: &[u8; 512],
    vhf: bool,
    level: u8,
) -> Result<Thresholds, u32> {
    if level > 9 {
        return Err(0x5102);
    }
    if level == 0 {
        return Ok(Thresholds::disabled());
    }

    let base = if vhf { 0x60 } else { 0 };
    let mut raw = [0u32; 6];
    let mut zero = true;
    let mut erased = true;
    let mut index = 0;
    while index < raw.len() {
        raw[index] = block[base + index * 16 + level as usize] as u32;
        zero = zero && raw[index] == 0;
        erased = erased && raw[index] == 255;
        index += 1;
    }
    if zero || erased {
        return Err(0x5103);
    }

    let open_noise = minimum(raw[2], 127);
    let close_noise = maximum(open_noise, minimum(raw[3], 127));
    let close_rssi = minimum(raw[0], raw[1]);
    let close_glitch = maximum(raw[5], raw[4]);

    Ok(Thresholds {
        open_rssi: raw[0],
        close_rssi,
        open_noise,
        close_noise,
        open_glitch: raw[5],
        close_glitch,
        adjusted: close_rssi != raw[1]
            || open_noise != raw[2]
            || close_noise != raw[3]
            || close_glitch != raw[4],
    })
}

const _: () = {
    assert!(matches!(decode(&[0; 512], true, 1), Err(0x5103)));
    assert!(matches!(decode(&[255; 512], false, 9), Err(0x5103)));
    assert!(matches!(decode(&[0; 512], false, 10), Err(0x5102)));
    assert!(decode(&[255; 512], true, 0).is_ok());

    let mut block = [0u8; 512];
    let vhf = [100, 90, 40, 60, 90, 70];
    let uhf = [120, 110, 30, 50, 80, 60];
    let mut index = 0;
    while index < 6 {
        block[0x60 + index * 16 + 3] = vhf[index];
        block[index * 16 + 3] = uhf[index];
        index += 1;
    }

    let v = match decode(&block, true, 3) {
        Ok(value) => value,
        Err(_) => panic!("VHF thresholds rejected"),
    };
    let u = match decode(&block, false, 3) {
        Ok(value) => value,
        Err(_) => panic!("UHF thresholds rejected"),
    };
    assert!(v.open_rssi == 100 && u.open_rssi == 120);
    assert!(v.open_noise == 40 && u.open_noise == 30);
    assert!(v.open_glitch == 70 && v.close_glitch == 90);
    assert!(!v.adjusted);

    block[0x60 + 16 + 3] = 110;
    block[0x60 + 32 + 3] = 200;
    block[0x60 + 48 + 3] = 20;
    block[0x60 + 64 + 3] = 10;
    let normalized = match decode(&block, true, 3) {
        Ok(value) => value,
        Err(_) => panic!("threshold normalization failed"),
    };
    assert!(normalized.close_rssi == 100);
    assert!(normalized.open_noise == 127);
    assert!(normalized.close_noise == 127);
    assert!(normalized.close_glitch == 70);
    assert!(normalized.adjusted);
};
const _: () = {
    use crate::app::squelch::{Gate, Metrics};

    let off = match Thresholds::am(0) {
        Ok(value) => value,
        Err(_) => panic!("AM SQL zero rejected"),
    };
    assert!(off.open_rssi == 0 && off.close_rssi == 0);
    assert!(off.open_noise == 127 && off.open_glitch == 255);
    assert!(matches!(Thresholds::am(10), Err(0x5102)));
    assert!(matches!(Thresholds::am(255), Err(0x5102)));

    let mut level = 1;
    while level <= 9 {
        let am = match Thresholds::am(level) {
            Ok(value) => value,
            Err(_) => panic!("AM SQL level rejected"),
        };
        assert!(am.open_rssi == 160 + (level as u32 - 1) * 16);
        assert!(am.close_rssi + 8 == am.open_rssi);
        assert!(am.open_rssi <= 511);
        assert!(am.open_noise == 127 && am.close_noise == 127);
        assert!(am.open_glitch == 255 && am.close_glitch == 255);
        assert!(!am.adjusted);

        // A baseline inside the reported range cannot open this scale.
        let baseline = Metrics { rssi: 147, noise: 65, glitch: 151 };
        let mut gate = Gate::new();
        let mut now = 0;
        while now <= 400 {
            assert!(!gate.update(now, baseline, am, false, false));
            assert!(!gate.is_open());
            now += 20;
        }

        // Even maximum FM noise/glitch indicators cannot prevent AM
        // opening after its RSSI condition persists for sixty ticks.
        let strong = Metrics {
            rssi: am.open_rssi, noise: 127, glitch: 255,
        };
        assert!(!gate.update(500, strong, am, false, false));
        assert!(!gate.update(520, strong, am, false, false));
        assert!(!gate.update(540, strong, am, false, false));
        assert!(gate.update(560, strong, am, false, false));
        assert!(gate.is_open());

        // Equality at the close threshold retains an established carrier.
        let held = Metrics {
            rssi: am.close_rssi, noise: 127, glitch: 255,
        };
        assert!(gate.update(580, held, am, false, false));

        let lost = Metrics {
            rssi: am.close_rssi - 1, noise: 0, glitch: 0,
        };
        now = 600;
        while now < 720 {
            assert!(gate.update(now, lost, am, false, false));
            now += 20;
        }
        assert!(!gate.update(720, lost, am, false, false));
        assert!(!gate.is_open());

        // Monitor bypass is audio admission, not a carrier observation.
        assert!(gate.update(740, baseline, am, true, false));
        assert!(!gate.is_open());
        assert!(!gate.update(760, strong, am, false, false));
        assert!(!gate.update(780, strong, am, false, false));
        assert!(!gate.update(800, strong, am, false, false));
        assert!(gate.update(820, strong, am, false, false));

        // PTT and a stale sample gap still discard carrier history.
        assert!(!gate.update(840, strong, am, false, true));
        assert!(!gate.is_open());
        assert!(!gate.update(860, strong, am, false, false));
        assert!(gate.update(920, strong, am, false, false));
        assert!(!gate.update(1101, strong, am, false, false));
        assert!(!gate.is_open());

        level += 1;
    }
};

const _: () = {
    // AM policy has no dependency on a particular factory threshold row.
    // These selector fixtures do not bypass Memory's acquisition prerequisite.
    let frequencies = [
        50_000_000, 174_000_000, 280_000_000,
        433_700_000, 840_000_000, 1_300_000_000,
    ];
    let mut index = 0;
    while index < frequencies.len() {
        let value = match select(&[0; 512], frequencies[index], 3, true) {
            Ok(value) => value,
            Err(_) => panic!("AM threshold selection rejected"),
        };
        assert!(value.open_rssi == 192 && value.close_rssi == 184);
        assert!(matches!(
            select(&[0; 512], frequencies[index], 3, false),
            Err(0x5103),
        ));
        index += 1;
    }

    let invalid = [
        0, 49_999_990, 630_000_000,
        839_999_990, 1_300_000_010, 433_700_001,
    ];
    index = 0;
    while index < invalid.len() {
        assert!(matches!(
            select(&[0; 512], invalid[index], 3, true), Err(0x5101),
        ));
        assert!(matches!(
            select(&[0; 512], invalid[index], 3, false), Err(0x5101),
        ));
        index += 1;
    }
    assert!(matches!(
        select(&[0; 512], 433_700_000, 10, true), Err(0x5102),
    ));

    // FM still reads all six columns from the selected factory table.
    let mut block = [0u8; 512];
    let vhf = [100, 90, 40, 60, 90, 70];
    let uhf = [120, 110, 30, 50, 80, 60];
    index = 0;
    while index < 6 {
        block[0x60 + index * 16 + 3] = vhf[index];
        block[index * 16 + 3] = uhf[index];
        index += 1;
    }

    let frequencies = [145_500_000, 174_000_000, 174_000_010, 433_700_000];
    index = 0;
    while index < frequencies.len() {
        let frequency = frequencies[index];
        let expected = match decode(&block, frequency <= 174_000_000, 3) {
            Ok(value) => value,
            Err(_) => panic!("FM factory fixture rejected"),
        };
        let actual = match select(&block, frequency, 3, false) {
            Ok(value) => value,
            Err(_) => panic!("FM selector rejected"),
        };
        assert!(actual.open_rssi == expected.open_rssi);
        assert!(actual.close_rssi == expected.close_rssi);
        assert!(actual.open_noise == expected.open_noise);
        assert!(actual.close_noise == expected.close_noise);
        assert!(actual.open_glitch == expected.open_glitch);
        assert!(actual.close_glitch == expected.close_glitch);
        assert!(actual.adjusted == expected.adjusted);
        index += 1;
    }
};
