use crate::{
    Context, Error,
    test_support::{run_async, sample_state},
};
use std::time::Duration;

#[test]
fn cancellation_notifies_context_and_stales_client_handles() {
    run_async(async {
        let state = sample_state("switch.fan", "on");
        let ctx = Context::with_seeded_states([state.clone()]);
        let ha = ctx.home_assistant();
        let cancelled = ctx.cancelled();

        ctx.cancel_generation();

        tokio::time::timeout(Duration::from_millis(50), cancelled)
            .await
            .expect("cancellation future should become ready");

        assert!(matches!(
            ha.get_state_raw(&state.entity_id).await,
            Err(Error::Cancelled)
        ));
    });
}
