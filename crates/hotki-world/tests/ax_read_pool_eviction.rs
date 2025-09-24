use std::{
    thread::sleep,
    time::{Duration, Instant},
};

use hotki_world::{test_api as world_test, test_support::test_serial_guard};

fn wait_until(mut cond: impl FnMut() -> bool, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(20));
    }
}

#[test]
fn cache_bounds_and_expires_entries() {
    let _guard = test_serial_guard();
    world_test::clear();
    world_test::ensure_ax_pool_inited();
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_async_only(true);

    let pid = 321;
    let total = 2_600u32;

    for id in 0..total {
        world_test::set_ax_title(id, &format!("TITLE-{id}"));
        assert!(world_test::ax_pool_schedule_title(pid, id).is_none());
    }

    // Allow workers to populate the cache with many distinct entries.
    sleep(Duration::from_millis(1_500));

    let (titles, props) = world_test::ax_pool_cache_usage();
    assert!(
        titles <= 2_048,
        "expected title cache to stay bounded, observed {titles}",
    );
    assert_eq!(props, 0, "props cache should remain empty in this test");

    // Wait past the TTL so stale entries should be evicted on the next prune.
    sleep(Duration::from_millis(3_300));

    let cleared = wait_until(
        || {
            let (t_after, _) = world_test::ax_pool_cache_usage();
            t_after <= 4
        },
        600,
    );
    assert!(
        cleared,
        "expected expired cache entries to be evicted promptly"
    );
}
