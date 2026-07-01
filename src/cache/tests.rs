use super::*;
use crate::{
    Context, Error,
    test_support::{run_async, sample_state},
};

#[test]
fn state_cache_get_state_raw_hits_and_misses() {
    run_async(async {
        let state = sample_state("light.office", "on");
        let ctx = Context::with_seeded_states([state.clone()]);
        let ha = ctx.home_assistant();

        assert_eq!(ha.get_state_raw(&state.entity_id).await.unwrap(), state);

        let missing = EntityId::new("light.missing").unwrap();
        assert!(matches!(
            ha.get_state_raw(&missing).await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == missing
        ));
    });
}

#[test]
fn cache_state_and_remove_cached_state_update_generation_cache() {
    run_async(async {
        let ctx = Context::new_generation();
        let ha = ctx.home_assistant();
        let state = sample_state("sensor.temperature", "21.5");
        let entity_id = state.entity_id.clone();

        ha.cache_state(state.clone()).unwrap();
        assert_eq!(ha.get_state_raw(&entity_id).await.unwrap(), state);
        assert!(ha.remove_cached_state(&entity_id).unwrap().is_some());
        assert!(matches!(
            ha.get_state_raw(&entity_id).await,
            Err(Error::EntityNotFound(missing)) if missing == entity_id
        ));
    });
}
