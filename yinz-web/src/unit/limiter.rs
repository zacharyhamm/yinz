use super::*;
#[test]
fn thresholds_escalation_reset_global_and_bounded_history() {
    let mut l = Limiter::default();
    for _ in 0..5 {
        l.admit("a", 1).unwrap();
        l.failure("a", 1);
    }
    assert_eq!(l.admit("a", 1).unwrap_err().retry_after_s, 60);
    assert!(l.admit("b", 1).is_ok());
    for _ in 0..5 {
        l.failure("a", 60001);
    }
    assert_eq!(l.admit("a", 60001).unwrap_err().retry_after_s, 120);
    l.success("a", 60001);
    assert!(l.admit("a", 60001).is_ok());
    for i in 0..20 {
        l.failure(&format!("b{i}"), 60001);
    }
    assert_eq!(l.admit("unrelated", 60001).unwrap_err().retry_after_s, 900);
    assert!(l.api("existing", i64::MAX, 60001).is_ok());
    for i in 0..3000 {
        l.failure(&format!("c{i}"), 1000000);
    }
    assert!(l.clients.len() <= 1024);
    assert!(l.overflow.failures.len() <= 5);
    assert!(l.global.len() < 30);
}
#[test]
fn bucket_refill_rounding_rollback_and_independence() {
    let mut b = Bucket::new(1000, 20, 40);
    for _ in 0..40 {
        b.spend(1000).unwrap();
    }
    assert_eq!(b.spend(1000).unwrap_err().retry_after_s, 1);
    assert!(b.spend(900).is_err());
    assert!(b.spend(1049).is_err());
    b.spend(1050).unwrap();
    let mut l = Limiter::default();
    for _ in 0..100 {
        l.public(0).unwrap();
    }
    assert!(l.public(0).is_err());
    l.public(20).unwrap();
    for _ in 0..40 {
        l.api("one", 10000, 0).unwrap();
    }
    assert!(l.api("one", 10000, 0).is_err());
    l.api("two", 10000, 0).unwrap();
    l.remove("one");
    l.api("one", 10000, 0).unwrap();
}
