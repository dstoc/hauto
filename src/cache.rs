use crate::{client::GenerationState, entity::EntityId, state::EntityState};

/// Read-only access to the current Home Assistant state cache.
///
/// `StateCache` is intentionally a thin wrapper around the crate's internal
/// generation cache. It lets typed entity handles synchronously decode cached
/// state without exposing mutation APIs. The view belongs to one connection
/// generation and is distinct from an explicit Home Assistant read. A lookup
/// can return no value when the entity is missing; `unknown` and `unavailable`
/// remain present states and are decoded by availability-aware handles.
pub struct StateCache<'a> {
    pub(crate) generation: &'a GenerationState,
}

impl<'a> StateCache<'a> {
    #[allow(dead_code)]
    pub(crate) fn new(generation: &'a GenerationState) -> Self {
        Self { generation }
    }

    pub(crate) fn raw_state(&self, entity_id: &EntityId) -> Option<EntityState> {
        self.generation.cached_state(entity_id)
    }
}

#[cfg(test)]
mod tests;
