use crate::{
    Context, EntityId,
    test_support::{run_async, sample_state},
};

#[test]
fn state_change_stream_filters_after_cache_update() {
    run_async(async {
        let ctx = Context::new_generation();
        let target = EntityId::new("binary_sensor.door").unwrap();
        let other = sample_state("binary_sensor.window", "on");
        let state = sample_state("binary_sensor.door", "on");
        let mut changes = ctx.state_changes(&target);

        ctx.home_assistant().cache_state(other).unwrap();
        ctx.home_assistant().cache_state(state.clone()).unwrap();

        let event = changes.next().await.unwrap().unwrap();
        assert_eq!(event.entity_id, target);
        assert_eq!(
            ctx.home_assistant().get_state_raw(&target).await.unwrap(),
            state
        );
    });
}
