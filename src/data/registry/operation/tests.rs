use super::*;

#[async_std::test]
async fn finish_event_observes_committed_terminal_and_no_active_snapshot() {
    let bus = EventBus::new(8);
    let mut receiver = bus.subscribe();
    let registry = OperationRegistry::new(bus);
    let handle = registry.begin("test".into(), vec![]);
    let id = handle.id();
    handle.finish(Outcome::Completed);
    while let Ok(event) = receiver.try_recv() {
        if let crate::bus::event::CoreEvent::Operation(message) = event {
            if matches!(message.data, message::event::OperationEvent::Finish(_)) {
                let view = registry.reconcile();
                assert!(view.active.is_empty());
                assert_eq!(view.completed[0].snapshot.id, id);
                assert!(matches!(view.completed[0].outcome, Outcome::Completed));
                assert!(matches!(
                    view.completed[0].snapshot.status,
                    LifeCycle::Completed
                ));
                return;
            }
        }
    }
    panic!("missing finish event");
}

#[test]
fn overflow_and_resubscription_reconcile_bounded_results_without_blocking_begin() {
    let bus = EventBus::new(1);
    let _receiver = bus.subscribe();
    let registry = OperationRegistry::with_history(bus.clone(), 2, Duration::from_secs(60));
    for _ in 0..1000 {
        registry
            .begin("completed".into(), vec![])
            .finish(Outcome::Failed);
    }
    let _resubscribed = bus.subscribe();
    let live = registry.begin("live".into(), vec![]);
    let view = registry.reconcile();
    assert_eq!(view.active.len(), 1);
    assert_eq!(view.active[0].id, live.id());
    assert_eq!(view.completed.len(), 2);
    assert!(view
        .completed
        .iter()
        .all(|result| matches!(result.outcome, Outcome::Failed)));
}

#[test]
fn panic_guard_records_failed_and_normal_drop_records_cancelled() {
    let registry = OperationRegistry::new(EventBus::new(4));
    let handle = registry.begin("panic".into(), vec![]);
    let id = handle.id();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _handle = handle;
        panic!("fixture");
    }));
    drop(registry.begin("drop".into(), vec![]));
    let view = registry.reconcile();
    assert!(view.active.is_empty());
    assert_eq!(view.completed[0].snapshot.id, id);
    assert!(matches!(view.completed[0].outcome, Outcome::Failed));
    assert!(matches!(view.completed[1].outcome, Outcome::Cancelled));
}

#[test]
fn zero_history_retention_never_leaves_a_finished_operation_active() {
    let registry = OperationRegistry::with_history(EventBus::new(1), 0, Duration::ZERO);
    registry
        .begin("test".into(), vec![])
        .finish(Outcome::Completed);
    let view = registry.reconcile();
    assert!(view.active.is_empty());
    assert!(view.completed.is_empty());
}

#[async_std::test]
async fn owned_future_panic_records_failed_even_when_future_drops_after_unwind_is_caught() {
    let registry = OperationRegistry::new(EventBus::new(1));
    let handle = registry.begin("worker".into(), vec![]).owned_worker();
    let task = registry
        .tasks
        .spawn("operation worker", None, async move {
            let _handle = handle;
            panic!("fixture");
        })
        .unwrap();
    assert!(task.await.is_err());
    let view = registry.reconcile();
    assert!(view.active.is_empty());
    assert!(matches!(view.completed[0].outcome, Outcome::Failed));
}
