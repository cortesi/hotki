//! Sanity checks for placement counter exports.

#[test]
fn place_counter_exports_accessible() {
    mac_winops::placement_counters_reset();
    let snapshot = mac_winops::placement_counters_snapshot();
    assert_eq!(snapshot.primary.attempts, 0);
}
