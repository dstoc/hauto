use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde_json::Value;
use tokio::sync::{OnceCell, broadcast, watch};

pub use crate::streams::{EventStreamError, RawEventStream, StateChangeStream};

use crate::{
    Error, RestStateRequest, RestStateTransport, Result, WsTransport,
    discovery::{AreaId, AreaMembership, CatalogSnapshot},
    entity::EntityId,
    map_delete_state_response, map_set_state_response,
    rest::ReqwestRestStateTransport,
    service_entity,
    state::{DeleteStateResult, EntityState, SetStateResult, StateChangedEvent, StateWrite},
    validate_domain_service, wait_cancelled,
};

#[derive(Clone)]
pub struct HomeAssistantClient {
    pub(crate) generation: Arc<GenerationState>,
    rest_states: Option<Arc<dyn RestStateTransport>>,
    ws: Option<Arc<WsTransport>>,
}

impl fmt::Debug for HomeAssistantClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HomeAssistantClient")
            .field("generation", &self.generation)
            .field("has_rest_states_transport", &self.rest_states.is_some())
            .field("has_ws_transport", &self.ws.is_some())
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
            ws: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_generation_with_rest_states(
        rest_base_url: impl AsRef<str>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            generation: Arc::new(GenerationState::new([])),
            rest_states: Some(Arc::new(ReqwestRestStateTransport::new(
                rest_base_url,
                access_token,
            )?)),
            ws: None,
        })
    }

    pub(crate) async fn new_generation_with_websocket_and_rest_states(
        rest_base_url: impl AsRef<str>,
        websocket_url: impl AsRef<str>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        let access_token = access_token.into();
        let generation = Arc::new(GenerationState::new([]));
        let ws =
            WsTransport::connect(websocket_url, access_token.clone(), generation.clone()).await?;
        Ok(Self {
            generation,
            rest_states: Some(Arc::new(ReqwestRestStateTransport::new(
                rest_base_url,
                access_token,
            )?)),
            ws: Some(ws),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_seeded_states(states: impl IntoIterator<Item = EntityState>) -> Self {
        Self {
            generation: Arc::new(GenerationState::new(states)),
            rest_states: None,
            ws: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_rest_states_transport(transport: impl RestStateTransport) -> Self {
        Self {
            generation: Arc::new(GenerationState::new([])),
            rest_states: Some(Arc::new(transport)),
            ws: None,
        }
    }

    #[cfg(test)]
    pub(crate) async fn connect_websocket_generation(
        websocket_url: impl AsRef<str>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        let generation = Arc::new(GenerationState::new([]));
        let ws = WsTransport::connect(websocket_url, access_token, generation.clone()).await?;
        Ok(Self {
            generation,
            rest_states: None,
            ws: Some(ws),
        })
    }

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
        data: Value,
    ) -> Result<Value> {
        self.ensure_generation_active()?;
        validate_domain_service(domain, service)?;
        let Some(transport) = &self.ws else {
            return Err(Error::NotImplemented(
                "HomeAssistantClient::call_service_raw",
            ));
        };

        transport.call_service_raw(domain, service, data).await
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
        let Some(transport) = &self.ws else {
            return Err(Error::NotImplemented("HomeAssistantClient::command_raw"));
        };
        transport.command_raw(command).await
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
        let Some(transport) = &self.ws else {
            return Ok(RawEventStream::placeholder());
        };
        let event_type = _event_type.map(str::to_string);
        transport.subscribe_events(event_type.as_deref()).await?;
        Ok(RawEventStream::new(
            self.generation.raw_events.subscribe(),
            event_type,
        ))
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

    pub(crate) async fn refresh_states_from_websocket(&self) -> Result<Vec<EntityState>> {
        self.ensure_generation_active()?;
        let Some(transport) = &self.ws else {
            return Err(Error::NotImplemented(
                "HomeAssistantClient::refresh_states_from_websocket",
            ));
        };
        let value = transport.get_states().await?;
        let states: Vec<EntityState> = serde_json::from_value(value).map_err(|error| {
            Error::Connection(format!("get_states response could not be decoded: {error}"))
        })?;
        for state in &states {
            self.generation.cache_state(state.clone());
        }
        Ok(states)
    }

    pub(crate) async fn subscribe_state_changed_events(&self) -> Result<RawEventStream> {
        self.subscribe_events_raw(Some("state_changed")).await
    }

    pub(crate) async fn discovery_catalog(&self) -> Result<Arc<CatalogSnapshot>> {
        self.ensure_generation_active()?;
        let mut cancelled = self.cancelled_receiver();
        let snapshot = tokio::select! {
            biased;
            snapshot = self.generation.discovery_catalog.get_or_try_init(|| async {
                self.ensure_generation_active()?;
                let Some(transport) = &self.ws else {
                    return Err(Error::NotImplemented("HomeAssistantClient::entity_catalog"));
                };
                let (areas, entities) = tokio::try_join!(
                    transport.list_areas(),
                    transport.list_entities_for_display()
                )?;
                self.ensure_generation_active()?;
                Ok(Arc::new(CatalogSnapshot::from_responses(
                    areas,
                    entities,
                    &self.generation,
                )?))
            }) => snapshot?,
            () = wait_cancelled(&mut cancelled) => return Err(Error::Cancelled),
        };
        self.ensure_generation_active()?;
        Ok(snapshot.clone())
    }

    pub(crate) async fn discovery_entities_in(
        &self,
        area_id: &AreaId,
    ) -> Result<Arc<AreaMembership>> {
        self.ensure_generation_active()?;
        let cell = {
            let mut memberships = self.generation.area_memberships.lock().await;
            memberships
                .entry(area_id.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let mut cancelled = self.cancelled_receiver();
        let membership = tokio::select! {
            biased;
            membership = cell.get_or_try_init(|| async {
                self.ensure_generation_active()?;
                let Some(transport) = &self.ws else {
                    return Err(Error::NotImplemented("HomeAssistantClient::entities_in"));
                };
                let response = transport.extract_area(area_id).await?;
                self.ensure_generation_active()?;
                Ok(Arc::new(response.referenced_entities.into_iter().collect()))
            }) => membership?,
            () = wait_cancelled(&mut cancelled) => return Err(Error::Cancelled),
        };
        self.ensure_generation_active()?;
        Ok(membership.clone())
    }
}

pub(crate) static NEXT_GENERATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(crate) struct GenerationState {
    id: u64,
    is_cancelled: AtomicBool,
    cancelled: watch::Sender<bool>,
    state_cache: Mutex<HashMap<EntityId, EntityState>>,
    discovery_catalog: OnceCell<Arc<CatalogSnapshot>>,
    area_memberships: tokio::sync::Mutex<HashMap<AreaId, Arc<OnceCell<Arc<AreaMembership>>>>>,
    pub(crate) state_changes: broadcast::Sender<StateChangedEvent>,
    pub(crate) raw_events: broadcast::Sender<Value>,
}

impl GenerationState {
    fn new(states: impl IntoIterator<Item = EntityState>) -> Self {
        let (cancelled, _receiver) = watch::channel(false);
        let (state_changes, _receiver) = broadcast::channel(64);
        let (raw_events, _receiver) = broadcast::channel(64);
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
            discovery_catalog: OnceCell::new(),
            area_memberships: tokio::sync::Mutex::new(HashMap::new()),
            state_changes,
            raw_events,
        }
    }

    pub(crate) fn cancel(&self) {
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

    #[allow(dead_code)]
    pub(crate) fn cache_state(&self, state: EntityState) {
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

    #[allow(dead_code)]
    pub(crate) fn remove_cached_state(&self, entity_id: &EntityId) -> Option<EntityState> {
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

    #[allow(dead_code)]
    pub(crate) fn apply_state_changed_event(&self, event: StateChangedEvent) {
        {
            let mut cache = self.state_cache.lock().expect("state cache lock poisoned");
            match &event.new_state {
                Some(state) => {
                    cache.insert(event.entity_id.clone(), state.clone());
                }
                None => {
                    cache.remove(&event.entity_id);
                }
            }
        }
        let _ = self.state_changes.send(event);
    }
}
