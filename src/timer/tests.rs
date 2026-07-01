use crate::{Context, Error, TimeoutResult, test_support::run_async};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

#[test]
fn sleep_returns_cancelled_when_generation_is_cancelled() {
    run_async(async {
        let ctx = Context::new_generation();
        ctx.cancel_generation();

        assert!(matches!(
            ctx.sleep(Duration::from_secs(60)).await,
            Err(Error::Cancelled)
        ));
    });
}
#[test]
fn timeout_reports_completed_and_timed_out() {
    run_async(async {
        let ctx = Context::new_generation();

        assert_eq!(
            ctx.timeout(Duration::from_secs(1), async { Ok(5) })
                .await
                .unwrap(),
            TimeoutResult::Completed(5)
        );
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(5)
            })
            .await
            .unwrap(),
            TimeoutResult::TimedOut
        );
    });
}
#[test]
fn spawn_handle_awaits_task_result() {
    run_async(async {
        let ctx = Context::new_generation();
        let handle = ctx.spawn(async { Ok(42) });

        assert_eq!(handle.await.unwrap(), 42);
    });
}
#[test]
fn run_after_can_complete_or_be_cancelled_without_dropping_task() {
    run_async(async {
        let ctx = Context::new_generation();
        let handle = ctx.run_after(Duration::from_millis(1), async { Ok("done") });
        assert_eq!(handle.await.unwrap(), "done");

        let mut handle = ctx.run_after(Duration::from_secs(60), async { Ok("late") });
        handle.cancel().await.unwrap();
        handle.cancel().await.unwrap();
        assert!(matches!(handle.await, Err(Error::Cancelled)));
    });
}
#[test]
fn run_after_cancel_waits_for_started_future_to_stop() {
    run_async(async {
        struct StopFlag(Arc<AtomicBool>);

        impl Drop for StopFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let ctx = Context::new_generation();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_for_task = stopped.clone();
        let (started_tx, mut started_rx) = watch::channel(false);
        let mut handle = ctx.run_after(Duration::from_millis(1), async move {
            let _stop = StopFlag(stopped_for_task);
            let _ = started_tx.send(true);
            std::future::pending::<()>().await;
            Ok(())
        });

        while !*started_rx.borrow() {
            started_rx.changed().await.unwrap();
        }

        handle.cancel().await.unwrap();
        assert!(stopped.load(Ordering::Acquire));
        assert!(matches!(handle.await, Err(Error::Cancelled)));
    });
}
