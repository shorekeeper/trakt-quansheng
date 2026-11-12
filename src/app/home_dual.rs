//! Two-profile Home with selected TX and temporary RX identification.
//!
//! The header distinguishes the selected transmit VFO from the current
//! settled receiver. Each block retains its RX frequency and shows the
//! effective TX destination, route mark, modulation, and RX signaling below.
//! Carrier is independent of signaling-controlled speaker admission.
//!
//! Panel supplies normalized requests for both destinations. These same
//! requests are used by RF service and settling checks. An unselected CW
//! destination retains its listen-only behavior and has no TX projection.
//!
//! Block origins are 11 and 35, separated by 24 pixels. Large frequencies
//! use the common native digit renderer; their compact ten-hertz suffix
//! fits without dropping a significant MHz digit. The footer starts at 58.
//! Rendering neither retunes RF nor changes watch timers or saved settings.

use super::{appearance::Settings as Appearance, dual::Watch, operation::Operation};
use crate::drivers::{
    adc::Reading,
    bk4829::{receive::{Phase, Request, View}, transmit::View as TxView},
};
use crate::storage::View as MemoryView;
use crate::ui::{font::Size, widgets, Frame};

#[inline(never)]
pub(super) fn render(
    frame: &mut Frame,
    watch: &Watch,
    operation: &Operation,
    requests: [Request; 2],
    receiver: View,
    transmitter: TxView,
    battery: Reading,
    memory: MemoryView,
    appearance: Appearance,
    dirty: bool,
    slope: crate::slope::Settings,
    vox_enabled: bool,
    vox_status: &[u8],
) {
    frame.clear();
    frame.text_size(2, 1, b"DW", Size::Small);
    frame.text_size(
        18, 1, &[b'T', b'X', b':', b'A' + operation.active as u8], Size::Small,
    );

    let listening = watch.listening();
    let request = requests[listening];
    let settled = matches!(receiver.phase, Phase::Receive)
        && receiver.frequency == request.frequency
        && receiver.narrow == request.narrow
        && receiver.am == request.am
        && receiver.rx_signal == request.rx_signal;

    frame.text_size(
        42, 1,
        &[b'R', b'X', b':', if settled { b'A' + listening as u8 } else { b'-' }],
        Size::Small,
    );
    if dirty {
        frame.text_size(60, 1, b"*", Size::Small);
    }
    if vox_enabled {
        frame.text_size(64, 1, b"VOX", Size::Small);
    }

    let voltage = super::power::millivolts(battery.average, memory.calibration);
    match appearance.battery {
        0 => widgets::battery(frame, voltage),
        1 => widgets::voltage(frame, 76, 1, voltage, Size::Small),
        _ => {}
    }
    widgets::badge(
        frame, 103, 0,
        if settled && receiver.signal { b"SIG" } else { b"DW" },
    );
    frame.fill(0, 8, 128, 1, true);

    for index in 0..2 {
        let profile = operation.vfo_profile(index);
        let request = requests[index];
        let cw = operation.vfo_cw(index);
        let y = 11 + index as i16 * 24;

        frame.text_size(2, y, &[b'A' + index as u8], Size::Small);
        if index == operation.active {
            frame.text_size(2, y + 8, b">", Size::Small);
        }
        widgets::frequency(frame, 8, y, profile.tune.frequency, Size::Large);

        if cw {
            frame.text_size(2, y + 17, b"CW LISTEN ONLY", Size::Small);
            continue;
        }

        frame.text_size(2, y + 17, b"TX", Size::Small);
        match crate::slope::resolve(profile, slope) {
            Ok(frequency) => widgets::frequency(
                frame, 14, y + 17,
                if profile.tune.am { profile.tune.frequency } else { frequency },
                Size::Small,
            ),
            Err(0x700C | 0x700F) => frame.text_size(14, y + 17, b"OFF", Size::Small),
            Err(_) => frame.text_size(14, y + 17, b"OUT OF BAND", Size::Small),
        }

        let route: &[u8] = match profile.transmit {
            crate::profile::Tx::Simplex => b"=",
            crate::profile::Tx::Plus(_) => b"+",
            crate::profile::Tx::Minus(_) => b"-",
            crate::profile::Tx::Split(_) => b"SPL",
            crate::profile::Tx::ReceiveOnly => b"OFF",
        };
        frame.text_size(60, y + 17, &route[..1], Size::Small);
        frame.text_size(66, y + 17, if cfg!(feature = "fam")
            && profile.tune.am && slope.enabled
        {
            b"SC"
        } else {
            profile.tune.name()
        }, Size::Small);

        if request.rx_signal != 0 {
            frame.text_size(82, y + 17, b"R", Size::Small);
            frame.text_size(
                90, y + 17, &crate::tx_signal::text(request.rx_signal), Size::Small,
            );
        }
    }

    let status: &[u8] = if transmitter.error != 0 {
        super::tx_policy::reason(transmitter.error)
    } else if !transmitter.ham {
        b"HAM OFF / CHECK AUTHORIZATION"
    } else if transmitter.blocked {
        b"RELEASE PTT"
    } else if vox_enabled {
        vox_status
    } else {
        watch.status()
    };
    frame.text_size(2, 58, status, Size::Small);
}