use std::time::{Duration, Instant};

use hotki_world::test_api as world_test;

// Helper: wait until `cond()` returns true or timeout
fn wait_until(mut cond: impl FnMut() -> bool, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn deadline_drops_stale_title() {
    world_test::clear();
    world_test::ensure_ax_pool_inited();
    world_test::ax_pool_reset_metrics_and_cache();

    // Configure a title override and introduce an artificial delay longer than the
    // pool deadline (200ms) so the worker will drop the result.
    let pid = 42;
    let id = 1;
    world_test::set_ax_title(id, "DELAYED-TITLE");
    world_test::set_ax_async_only(true);
    world_test::set_ax_delay_title_ms(400);

    // Schedule the title read; the immediate return must be None (cache miss).
    assert!(world_test::ax_pool_schedule_title(pid, id).is_none());

    // Wait well past the delay and confirm we observed a stale drop.
    assert!(wait_until(
        || world_test::ax_pool_stale_drop_count() >= 1,
        800
    ));
}

#[test]
fn global_concurrency_is_bounded() {
    world_test::clear();
    world_test::ensure_ax_pool_inited();
    world_test::ax_pool_reset_metrics_and_cache();

    // Ensure async path and set a small delay.
    world_test::set_ax_async_only(true);
    world_test::set_ax_delay_title_ms(20);

    // Seed distinct titles and schedule across 8 different PIDs so that we engage
    // multiple workers and exercise the global semaphore.
    // Use a single AX id override (1) so all title reads resolve via the override path.
    world_test::set_ax_title(1, "T-1");
    // Ensure we observe some concurrency (at least one in flight soon after scheduling).
    let _ = world_test::ax_pool_schedule_title(99, 1);
    assert!(wait_until(|| world_test::ax_pool_metrics().1 >= 1, 300));

    for pid in 100..108i32 {
        let _ = world_test::ax_pool_schedule_title(pid, 1);
    }

    // Wait until we observe concurrency (peak >= 2) and then validate the cap (<=4).
    assert!(wait_until(|| world_test::ax_pool_metrics().1 >= 2, 800));
    let (_current, peak) = world_test::ax_pool_metrics();
    assert!(peak <= 4, "observed peak inflight {} > 4", peak);

    // We don't assert cache results here to avoid time flakiness; metrics capture the bound.
}
