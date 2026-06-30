use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde_json::Value;
use tokio::sync::{broadcast, watch};

use crate::{
    DeleteStateResult, EntityId, EntityState, Error, RawEventStream, RestStateRequest,
    RestStateTransport, Result, SetStateResult, StateChangeStream, StateChangedEvent, StateWrite,
    map_delete_state_response, map_set_state_response, service_entity, validate_domain_service,
};

#[derive(Clone)]
pub struct HomeAssistantClient {
    pub(crate) generation: Arc<GenerationState>,
    rest_states: Option<Arc<dyn RestStateTransport>>,
}

impl fmt::Debug for HomeAssistantClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HomeAssistantClient")
            .field("generation", &self.generation)
            .field("has_rest_states_transport", &self.rest_states.is_some())
            .finish()
    }
}

impl Default for HomeAssistantClient {
    fn default() -> Self {
        Self::new_generation()
    }
}

impl HomeAssistantClient {
    pub(crate) fn new_generation() -> Self {
        Self {
            generation: Arc::new(GenerationState::new([])),
            rest_states: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_seeded_states(states: impl IntoIterator<Item = EntityState>) -> Self {
        Self {
            generation: Arc::new(GenerationState::new(states)),
            rest_states: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_rest_states_transport(transport: impl RestStateTransport) -> Self {
        Self {
            generation: Arc::new(GenerationState::new([])),
            rest_states: Some(Arc::new(transport)),
        }
    }

    #[cfg(test)]
    pub(crate) fn cancel_generation(&self) {
        self.generation.cancel();
    }

    #[cfg(test)]
    pub(crate) fn cache_state(&self, state: EntityState) -> Result<()> {
        self.ensure_generation_active()?;
        self.generation.cache_state(state);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn remove_cached_state(&self, entity_id: &EntityId) -> Result<Option<EntityState>> {
        self.ensure_generation_active()?;
        Ok(self.generation.remove_cached_state(entity_id))
    }

    pub async fn call_service_raw(
        &self,
        domain: &str,
        service: &str,
        _data: Value,
    ) -> Result<Value> {
        self.ensure_generation_active()?;
        validate_domain_service(domain, service)?;
        Err(Error::NotImplemented(
            "HomeAssistantClient::call_service_raw",
        ))
    }

    pub async fn command_raw(&self, command: Value) -> Result<Value> {
        self.ensure_generation_active()?;
        let object = command.as_object().ok_or_else(|| {
            Error::InvalidServiceOptions("raw commands must be JSON objects".to_string())
        })?;
        if object.contains_key("id") {
            return Err(Error::InvalidServiceOptions(
                "raw commands must not include caller-supplied `id`".to_string(),
            ));
        }
        match object.get("type") {
            Some(Value::String(value)) if !value.is_empty() => {}
            Some(Value::String(_)) => {
                return Err(Error::InvalidServiceOptions(
                    "raw commands require a non-empty string `type`".to_string(),
                ));
            }
            _ => {
                return Err(Error::InvalidServiceOptions(
                    "raw commands require a string `type`".to_string(),
                ));
            }
        }
        Err(Error::NotImplemented("HomeAssistantClient::command_raw"))
    }

    pub async fn set_state_raw(
        &self,
        entity_id: &EntityId,
        state: StateWrite,
    ) -> Result<SetStateResult> {
        state.validate()?;
        self.ensure_generation_active()?;
        let Some(transport) = &self.rest_states else {
            return Err(Error::NotImplemented("HomeAssistantClient::set_state_raw"));
        };

        match transport
            .send(RestStateRequest::set(entity_id.clone(), state))
            .await
        {
            Ok(response) => map_set_state_response(response),
            Err(error) if error.outcome_unknown => Err(Error::OutcomeUnknown(error.message)),
            Err(error) => Err(Error::Connection(error.message)),
        }
    }

    pub async fn delete_state_raw(&self, entity_id: &EntityId) -> Result<DeleteStateResult> {
        self.ensure_generation_active()?;
        let Some(transport) = &self.rest_states else {
            return Err(Error::NotImplemented(
                "HomeAssistantClient::delete_state_raw",
            ));
        };

        match transport
            .send(RestStateRequest::delete(entity_id.clone()))
            .await
        {
            Ok(response) => map_delete_state_response(response),
            Err(error) if error.outcome_unknown => Err(Error::OutcomeUnknown(error.message)),
            Err(error) => Err(Error::Connection(error.message)),
        }
    }

    pub async fn get_state_raw(&self, entity_id: &EntityId) -> Result<EntityState> {
        self.ensure_generation_active()?;
        self.generation
            .cached_state(entity_id)
            .ok_or_else(|| Error::EntityNotFound(entity_id.clone()))
    }

    pub async fn subscribe_state_changes(&self) -> Result<StateChangeStream> {
        self.ensure_generation_active()?;
        Ok(StateChangeStream::new(
            self.generation.state_changes.subscribe(),
            None,
        ))
    }

    pub async fn subscribe_events_raw(&self, _event_type: Option<&str>) -> Result<RawEventStream> {
        self.ensure_generation_active()?;
        Ok(RawEventStream::placeholder())
    }

    pub async fn turn_on(&self, entity_id: &EntityId) -> Result<Value> {
        self.call_service_raw("homeassistant", "turn_on", service_entity(entity_id))
            .await
    }

    pub async fn turn_off(&self, entity_id: &EntityId) -> Result<Value> {
        self.call_service_raw("homeassistant", "turn_off", service_entity(entity_id))
            .await
    }

    pub(crate) fn cancelled_receiver(&self) -> watch::Receiver<bool> {
        self.generation.cancelled.subscribe()
    }

    pub(crate) fn ensure_generation_active(&self) -> Result<()> {
        let _generation_id = self.generation.id;
        if self.generation.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub(crate) static NEXT_GENERATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(crate) struct GenerationState {
    id: u64,
    is_cancelled: AtomicBool,
    cancelled: watch::Sender<bool>,
    state_cache: Mutex<HashMap<EntityId, EntityState>>,
    pub(crate) state_changes: broadcast::Sender<StateChangedEvent>,
}

impl GenerationState {
    fn new(states: impl IntoIterator<Item = EntityState>) -> Self {
        let (cancelled, _receiver) = watch::channel(false);
        let (state_changes, _receiver) = broadcast::channel(64);
        Self {
            id: NEXT_GENERATION_ID.fetch_add(1, Ordering::Relaxed),
            is_cancelled: AtomicBool::new(false),
            cancelled,
            state_cache: Mutex::new(
                states
                    .into_iter()
                    .map(|state| (state.entity_id.clone(), state))
                    .collect(),
            ),
            state_changes,
        }
    }

    #[cfg(test)]
    fn cancel(&self) {
        self.is_cancelled.store(true, Ordering::Release);
        let _ = self.cancelled.send(true);
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn cached_state(&self, entity_id: &EntityId) -> Option<EntityState> {
        self.state_cache
            .lock()
            .expect("state cache lock poisoned")
            .get(entity_id)
            .cloned()
    }

    #[cfg(test)]
    fn cache_state(&self, state: EntityState) {
        let old_state = self
            .state_cache
            .lock()
            .expect("state cache lock poisoned")
            .insert(state.entity_id.clone(), state.clone());
        let _ = self.state_changes.send(StateChangedEvent {
            entity_id: state.entity_id.clone(),
            old_state,
            new_state: Some(state),
        });
    }

    #[cfg(test)]
    fn remove_cached_state(&self, entity_id: &EntityId) -> Option<EntityState> {
        let old_state = self
            .state_cache
            .lock()
            .expect("state cache lock poisoned")
            .remove(entity_id);
        let _ = self.state_changes.send(StateChangedEvent {
            entity_id: entity_id.clone(),
            old_state: old_state.clone(),
            new_state: None,
        });
        old_state
    }
}
