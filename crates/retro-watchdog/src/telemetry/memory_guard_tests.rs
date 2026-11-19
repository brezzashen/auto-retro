use super::*;

const GIB: f64 = 1_073_741_824.0;

fn reading(used: f64, free: f64) -> Reading { Reading { swap_total: 33.0 * GIB, swap_used: used * GIB, free: free * GIB } }

#[test]
fn parses_the_2026_11_04_readings() {
    let (total, used) = parse_swap("total = 33792.00M  used = 33117.75M  free = 674.25M  (encrypted)").unwrap();
    assert_eq!((total, used), (33792.0 * 1_048_576.0, 33117.75 * 1_048_576.0));
    let stat = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               15680.\nPages active:  1.\n";
    assert_eq!(parse_free(stat), Some(15680.0 * 16384.0));
    assert!(parse_swap("garbage").is_none());
}

#[test]
fn exhaustion_needs_both_full_swap_and_low_free_memory() {
    assert!(reading(33.1, 0.24).exhausted());
    assert!(!reading(20.0, 0.24).exhausted(), "swap below 24 GiB is not exhaustion");
    let normal = Reading { swap_total: 11.0 * GIB, swap_used: 10.15 * GIB, free: 0.15 * GIB };
    assert!(!normal.exhausted(), "12:21 normal load: 92% of a shrunken swap total is still not exhaustion");
    assert!(!reading(33.1, 4.0).exhausted(), "free memory above 1 GiB is not exhaustion");
    assert!(!Reading { swap_total: 0.0, swap_used: 0.0, free: 0.0 }.exhausted(), "no swap file yet");
}

#[test]
fn opens_once_after_sixty_seconds_then_relieves_every_ten_minutes() {
    let mut guard = MemoryGuard::default();
    let hot = reading(33.1, 0.3);
    assert_eq!(guard.step(1000.0, &hot), Step::Holding);
    assert_eq!(guard.step(1059.0, &hot), Step::Holding);
    assert_eq!(guard.step(1060.0, &hot), Step::Open("1060000".into()));
    assert_eq!(guard.step(1300.0, &hot), Step::Holding);
    assert_eq!(guard.step(1660.0, &hot), Step::Relieve("1060000".into()));
    assert_eq!(guard.step(1665.0, &reading(10.0, 0.3)), Step::Idle);
    assert_eq!(guard.step(1670.0, &hot), Step::Holding, "a new episode holds again before opening");
    assert_eq!(guard.step(1730.0, &hot), Step::Open("1730000".into()));
}

#[test]
fn one_relieved_sample_breaks_the_hold() {
    let mut guard = MemoryGuard::default();
    let hot = reading(33.1, 0.3);
    guard.step(0.0, &hot);
    assert_eq!(guard.step(30.0, &reading(33.1, 2.0)), Step::Idle);
    assert_eq!(guard.step(60.0, &hot), Step::Holding);
}

#[test]
fn reports_what_was_paged_during_the_hold() {
    let stat = |ins: u64, outs: u64| format!("Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:  15680.\nDecompressions:  1000.\nCompressions:  {}.\nPageins:  7.\nSwapins:  {ins}.\nSwapouts:  {outs}.\n", 2000 + outs);
    let first = parse_paging(&stat(100, 200)).unwrap();
    assert_eq!(first, Paging { swapins: 100.0 * 16384.0, swapouts: 200.0 * 16384.0, compressions: 2200.0 * 16384.0, decompressions: 1000.0 * 16384.0 });
    // 65 s later: 131,072 pages (2 GiB) swapped out, 6,554 pages (0.1 GiB) swapped in.
    let last = parse_paging(&stat(100 + 6554, 200 + 131_072)).unwrap();
    assert_eq!(paged(Some((1000.0, first)), 1065.0, Some(last)),
        json!({"window_s": 65.0, "swapouts_gib": 2.0, "swapins_gib": 0.1, "compressions_gib": 2.0, "decompressions_gib": 0.0}));
    assert_eq!(paged(None, 1065.0, Some(last)), Value::Null, "no hold, no window");
    assert_eq!(paged(Some((1000.0, first)), 1065.0, None), Value::Null, "an unparsable sample is not a zero");
    assert!(parse_paging("Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\n").is_none());
}
