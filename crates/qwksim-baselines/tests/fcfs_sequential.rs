//! Integration test for the **T1.7** acceptance criterion: three
//! arrivals at simulator-times 0, 10, 20 with service-time 5 each
//! complete at 5, 15, 25.

use qwksim_baselines::Fcfs;
use qwksim_resources::HpcPartitionAgent;

#[test]
fn arrivals_at_0_10_20_with_service_5_complete_at_5_15_25() {
    let mut f = Fcfs::new(HpcPartitionAgent::new(1, /* total_cores */ 1));

    let c0 = f.submit(0, /* cores */ 1, /* service_ns */ 5);
    let c1 = f.submit(10, 1, 5);
    let c2 = f.submit(20, 1, 5);

    assert_eq!(c0, 5);
    assert_eq!(c1, 15);
    assert_eq!(c2, 25);
}

#[test]
fn fcfs_preserves_arrival_order_under_contention() {
    // 1 core, all three jobs arrive at t = 0. FIFO means they run
    // back-to-back; total makespan = 3 × service.
    let mut f = Fcfs::new(HpcPartitionAgent::new(1, 1));
    assert_eq!(f.submit(0, 1, 7), 7);
    assert_eq!(f.submit(0, 1, 7), 14);
    assert_eq!(f.submit(0, 1, 7), 21);
}
