//! Host checks of the absent comparator used by CW firmware compositions.
//!
//! This production owner has no RF transport dependency. The test checks
//! its zero-sized state, rejected activation, and unavailable observations.
//! It does not execute a radio or establish behavior of the active driver.

#[path = "../src/drivers/bk4829/vox_off.rs"]
mod disabled;

#[test]
fn absent_comparator_cannot_activate_or_supply_voice_observations() {
    assert_eq!(core::mem::size_of::<disabled::Detector>(), 0);
    assert_eq!(disabled::MASK, 0);

    let mut detector = disabled::Detector::new();
    assert!(!detector.active());
    assert!(detector.matches(0));
    assert!(!detector.matches(37));
    assert_eq!(detector.start(100, 37), Err(0x7C02));
    assert_eq!(detector.start(100, 0), Err(0x7C02));
    detector.check().unwrap();

    detector.observe(100, 0x0010);
    detector.pump(110);
    assert!(detector.sample(110).is_none());
    assert!(!detector.active());
    detector.stop();
    assert!(detector.sample(120).is_none());
}