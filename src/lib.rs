//! A small Rust automation framework scaffold for Home Assistant.
//!
//! This crate currently defines the public surface proposed for `hauto`.
//! Runtime, transport, cache, and event fan-out behavior are intentionally
//! placeholder implementations for now.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    fmt,
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use thiserror::Error as ThisError;
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};
use url::Url;

pub type Result<T, E = Error> = std::result::Result<T, E>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("authentication failed: {0}")]
    Authentication(String),
    #[error("entity not found: {0}")]
    EntityNotFound(EntityId),
    #[error("invalid entity id `{value}`: {reason}")]
    InvalidEntityId { value: String, reason: String },
    #[error("invalid domain for `{entity_id}`: expected `{expected}`, got `{actual}`")]
    InvalidDomain {
        entity_id: EntityId,
        expected: &'static str,
        actual: String,
    },
    #[error("invalid state for `{entity_id}`: {reason}")]
    InvalidState { entity_id: EntityId, reason: String },
    #[error("service call rejected: {0}")]
    ServiceRejected(String),
    #[error("service call was not sent: {0}")]
    NotSent(String),
    #[error("operation outcome is unknown: {0}")]
    OutcomeUnknown(String),
    #[error("automation task failed: {0}")]
    AutomationTask(String),
    #[error("event stream error: {0:?}")]
    EventStream(EventStreamError),
    #[error("context was cancelled")]
    Cancelled,
    #[error("invalid service options: {0}")]
    InvalidServiceOptions(String),
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
}

#[derive(Clone)]
pub struct App {
    home_assistant_url: String,
    websocket_url: String,
    rest_states_url: String,
    access_token: String,
    registrations: Vec<AutomationRegistration>,
}

type AutomationRunner = Arc<dyn Fn(Context) -> BoxFuture<Result<()>> + Send + Sync + 'static>;

#[derive(Clone)]
struct AutomationRegistration {
    name: String,
    run: AutomationRunner,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("home_assistant_url", &self.home_assistant_url)
            .field("websocket_url", &self.websocket_url)
            .field("rest_states_url", &self.rest_states_url)
            .field("registrations", &self.automation_names())
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        let urls = HomeAssistantUrls::from_base_url(url.into());
        Self {
            home_assistant_url: urls.base_url,
            websocket_url: urls.websocket_url,
            rest_states_url: urls.rest_states_url,
            access_token: token.into(),
            registrations: Vec::new(),
        }
    }

    pub fn automation<A, F>(mut self, name: impl Into<String>, factory: F) -> Self
    where
        A: Automation,
        F: Fn() -> A + Send + Sync + 'static,
    {
        self.registrations.push(AutomationRegistration {
            name: name.into(),
            run: Arc::new(move |ctx| factory().run(ctx)),
        });
        self
    }

    pub fn automation_fn<F, Fut>(mut self, name: impl Into<String>, run: F) -> Self
    where
        F: Fn(Context) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registrations.push(AutomationRegistration {
            name: name.into(),
            run: Arc::new(move |ctx| Box::pin(run(ctx))),
        });
        self
    }

    pub fn automation_names(&self) -> Vec<&str> {
        self.registrations
            .iter()
            .map(|registration| registration.name.as_str())
            .collect()
    }

    pub async fn run(self) -> Result<()> {
        let _ctx = Context::new_generation();
        let registrations = self.registrations;
        for registration in &registrations {
            let _ = &registration.run;
        }
        let _ = (
            self.home_assistant_url,
            self.websocket_url,
            self.rest_states_url,
            self.access_token,
            registrations,
        );
        Err(Error::NotImplemented("App::run"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HomeAssistantUrls {
    base_url: String,
    websocket_url: String,
    rest_states_url: String,
}

impl HomeAssistantUrls {
    fn from_base_url(base_url: String) -> Self {
        let mut base = Url::parse(&base_url).unwrap_or_else(|error| {
            panic!("invalid Home Assistant base URL `{base_url}`: {error}")
        });
        match base.scheme() {
            "http" | "https" => {}
            scheme => panic!("Home Assistant base URL must use http or https, got `{scheme}`"),
        }
        base.set_query(None);
        base.set_fragment(None);

        let mut websocket = base.clone();
        websocket
            .set_scheme(match base.scheme() {
                "http" => "ws",
                "https" => "wss",
                _ => unreachable!("scheme checked above"),
            })
            .expect("ws/wss are valid URL schemes");
        websocket.set_path("/api/websocket");

        let mut states = base.clone();
        states.set_path("/api/states");

        Self {
            base_url: base.to_string().trim_end_matches('/').to_string(),
            websocket_url: websocket.to_string(),
            rest_states_url: states.to_string().trim_end_matches('/').to_string(),
        }
    }
}

pub trait Automation: Send + 'static {
    fn run(self, ctx: Context) -> BoxFuture<Result<()>>
    where
        Self: Sized;
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    home_assistant: HomeAssistantClient,
}

impl Context {
    pub(crate) fn new_generation() -> Self {
        Self {
            home_assistant: HomeAssistantClient::new_generation(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_seeded_states(states: impl IntoIterator<Item = EntityState>) -> Self {
        Self {
            home_assistant: HomeAssistantClient::with_seeded_states(states),
        }
    }

    #[cfg(test)]
    pub(crate) fn cancel_generation(&self) {
        self.home_assistant.cancel_generation();
    }

    pub fn home_assistant(&self) -> HomeAssistantClient {
        self.home_assistant.clone()
    }

    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut cancelled = self.home_assistant.cancelled_receiver();
        async move {
            if *cancelled.borrow() {
                return;
            }

            while cancelled.changed().await.is_ok() {
                if *cancelled.borrow() {
                    return;
                }
            }
        }
    }

    pub fn spawn<F, T>(&self, _future: F) -> TaskHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let mut cancelled = self.home_assistant.cancelled_receiver();
        let handle = tokio::spawn(async move {
            if *cancelled.borrow() {
                return Err(Error::Cancelled);
            }

            tokio::select! {
                result = _future => result,
                _ = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
            }
        });
        TaskHandle::from_join_handle(handle)
    }

    pub async fn sleep(&self, duration: Duration) -> Result<()> {
        self.home_assistant.ensure_generation_active()?;
        let mut cancelled = self.home_assistant.cancelled_receiver();
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
        }
    }

    pub async fn timeout<F, T>(&self, duration: Duration, future: F) -> Result<TimeoutResult<T>>
    where
        F: Future<Output = Result<T>> + Send,
        T: Send,
    {
        self.home_assistant.ensure_generation_active()?;
        let mut cancelled = self.home_assistant.cancelled_receiver();
        tokio::select! {
            result = future => result.map(TimeoutResult::Completed),
            () = tokio::time::sleep(duration) => Ok(TimeoutResult::TimedOut),
            () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
        }
    }

    pub fn run_after<F, T>(&self, duration: Duration, future: F) -> TimerHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let timer = Arc::new(TimerControl::new());
        let timer_for_task = timer.clone();
        let completion_for_task = timer.clone();
        let mut timer_cancelled = timer.subscribe();
        let mut cancelled = self.home_assistant.cancelled_receiver();
        let handle = tokio::spawn(async move {
            let _completion = TimerCompletionGuard(completion_for_task);
            let result = async move {
                if *cancelled.borrow() {
                    return Err(Error::Cancelled);
                }

                tokio::select! {
                    () = tokio::time::sleep(duration) => {}
                    () = wait_cancelled(&mut timer_cancelled) => return Err(Error::Cancelled),
                    () = wait_cancelled(&mut cancelled) => return Err(Error::Cancelled),
                }

                if timer_for_task.is_cancelled() {
                    return Err(Error::Cancelled);
                }

                tokio::select! {
                    result = future => result,
                    () = wait_cancelled(&mut timer_cancelled) => Err(Error::Cancelled),
                    () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
                }
            }
            .await;
            result
        });
        TimerHandle::from_join_handle(handle, timer)
    }

    pub fn state_changes(&self, entity: &EntityId) -> StateChangeStream {
        StateChangeStream::new(
            self.home_assistant.generation.state_changes.subscribe(),
            Some(entity.clone()),
        )
    }

    pub fn binary_sensor_changes(&self, sensor: &BinarySensor) -> StateChangeStream {
        self.state_changes(sensor.entity_id())
    }

    pub fn light_changes(&self, light: &Light) -> StateChangeStream {
        self.state_changes(light.entity_id())
    }
}

#[derive(Clone, Debug)]
pub struct HomeAssistantClient {
    generation: Arc<GenerationState>,
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
        }
    }

    #[cfg(test)]
    pub(crate) fn with_seeded_states(states: impl IntoIterator<Item = EntityState>) -> Self {
        Self {
            generation: Arc::new(GenerationState::new(states)),
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
        _domain: &str,
        _service: &str,
        _data: Value,
    ) -> Result<Value> {
        self.ensure_generation_active()?;
        Err(Error::NotImplemented(
            "HomeAssistantClient::call_service_raw",
        ))
    }

    pub async fn command_raw(&self, command: Value) -> Result<Value> {
        self.ensure_generation_active()?;
        if command.get("id").is_some() {
            return Err(Error::InvalidServiceOptions(
                "raw commands must not include caller-supplied `id`".to_string(),
            ));
        }
        Err(Error::NotImplemented("HomeAssistantClient::command_raw"))
    }

    pub async fn set_state_raw(
        &self,
        _entity_id: &EntityId,
        state: StateWrite,
    ) -> Result<SetStateResult> {
        state.validate()?;
        self.ensure_generation_active()?;
        Err(Error::NotImplemented("HomeAssistantClient::set_state_raw"))
    }

    pub async fn delete_state_raw(&self, _entity_id: &EntityId) -> Result<DeleteStateResult> {
        self.ensure_generation_active()?;
        Err(Error::NotImplemented(
            "HomeAssistantClient::delete_state_raw",
        ))
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

    fn cancelled_receiver(&self) -> watch::Receiver<bool> {
        self.generation.cancelled.subscribe()
    }

    fn ensure_generation_active(&self) -> Result<()> {
        let _generation_id = self.generation.id;
        if self.generation.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

static NEXT_GENERATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct GenerationState {
    id: u64,
    is_cancelled: AtomicBool,
    cancelled: watch::Sender<bool>,
    state_cache: Mutex<HashMap<EntityId, EntityState>>,
    state_changes: broadcast::Sender<StateChangedEvent>,
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

    fn cached_state(&self, entity_id: &EntityId) -> Option<EntityState> {
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntityId(String);

impl EntityId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_entity_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn domain(&self) -> &str {
        self.0
            .split_once('.')
            .expect("EntityId invariant guarantees one dot")
            .0
    }

    pub fn object_id(&self) -> &str {
        self.0
            .split_once('.')
            .expect("EntityId invariant guarantees one dot")
            .1
    }

    pub fn ensure_domain(&self, expected: &'static str) -> Result<()> {
        let actual = self.domain();
        if actual == expected {
            Ok(())
        } else {
            Err(Error::InvalidDomain {
                entity_id: self.clone(),
                expected,
                actual: actual.to_string(),
            })
        }
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for EntityId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for EntityId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<String> for EntityId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<EntityId> for String {
    fn from(value: EntityId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Light {
    entity_id: EntityId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    entity_id: EntityId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinarySensor {
    entity_id: EntityId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sensor<T> {
    entity_id: EntityId,
    _state: PhantomData<T>,
}

macro_rules! entity_handle {
    ($ty:ty, $domain:literal) => {
        impl $ty {
            pub fn new(entity_id: impl Into<String>) -> Result<Self> {
                let entity_id = EntityId::new(entity_id)?;
                entity_id.ensure_domain($domain)?;
                Ok(Self { entity_id })
            }

            pub fn entity_id(&self) -> &EntityId {
                &self.entity_id
            }

            pub async fn state(&self, ctx: &Context) -> Result<EntityState> {
                ctx.home_assistant().get_state_raw(&self.entity_id).await
            }
        }
    };
}

entity_handle!(Light, "light");
entity_handle!(Switch, "switch");
entity_handle!(BinarySensor, "binary_sensor");

impl<T> Sensor<T> {
    pub fn new(entity_id: impl Into<String>) -> Result<Self> {
        let entity_id = EntityId::new(entity_id)?;
        entity_id.ensure_domain("sensor")?;
        Ok(Self {
            entity_id,
            _state: PhantomData,
        })
    }

    pub fn entity_id(&self) -> &EntityId {
        &self.entity_id
    }

    pub async fn state(&self, ctx: &Context) -> Result<EntityState> {
        ctx.home_assistant().get_state_raw(&self.entity_id).await
    }
}

impl Light {
    pub async fn turn_on(&self, ctx: &Context, options: LightTurnOn) -> Result<Value> {
        options.validate()?;
        ctx.home_assistant()
            .call_service_raw(
                "light",
                "turn_on",
                options.into_service_data(&self.entity_id),
            )
            .await
    }

    pub async fn turn_off(&self, ctx: &Context, options: LightTurnOff) -> Result<Value> {
        ctx.home_assistant()
            .call_service_raw(
                "light",
                "turn_off",
                options.into_service_data(&self.entity_id),
            )
            .await
    }
}

impl Switch {
    pub async fn turn_on(&self, ctx: &Context) -> Result<Value> {
        ctx.home_assistant()
            .call_service_raw("switch", "turn_on", service_entity(&self.entity_id))
            .await
    }

    pub async fn turn_off(&self, ctx: &Context) -> Result<Value> {
        ctx.home_assistant()
            .call_service_raw("switch", "turn_off", service_entity(&self.entity_id))
            .await
    }
}

impl BinarySensor {
    pub fn wait_until<'a>(&'a self, ctx: &'a Context, target: BinaryState) -> StateWait<'a> {
        StateWait::new(ctx, self.entity_id.clone(), target)
    }

    pub fn wait_until_on<'a>(&'a self, ctx: &'a Context) -> StateWait<'a> {
        self.wait_until(ctx, BinaryState::On)
    }

    pub fn wait_until_off<'a>(&'a self, ctx: &'a Context) -> StateWait<'a> {
        self.wait_until(ctx, BinaryState::Off)
    }

    pub fn expect_state<'a>(
        &'a self,
        ctx: &'a Context,
        target: BinaryState,
    ) -> StateExpectation<'a> {
        StateExpectation::new(ctx, self.entity_id.clone(), target)
    }

    pub fn expect_on<'a>(&'a self, ctx: &'a Context) -> StateExpectation<'a> {
        self.expect_state(ctx, BinaryState::On)
    }

    pub fn expect_off<'a>(&'a self, ctx: &'a Context) -> StateExpectation<'a> {
        self.expect_state(ctx, BinaryState::Off)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryState {
    On,
    Off,
    Unknown,
    Unavailable,
}

impl BinaryState {
    pub fn decode(state: &str) -> Result<Self> {
        match state {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            "unknown" => Ok(Self::Unknown),
            "unavailable" => Ok(Self::Unavailable),
            other => Err(Error::InvalidServiceOptions(format!(
                "invalid binary state `{other}`"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: EntityId,
    pub state: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    pub last_changed: String,
    pub last_updated: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateWrite {
    pub state: String,
    pub attributes: Value,
}

impl StateWrite {
    pub fn new(state: impl Into<String>, attributes: Value) -> Result<Self> {
        let write = Self {
            state: state.into(),
            attributes,
        };
        write.validate()?;
        Ok(write)
    }

    pub fn validate(&self) -> Result<()> {
        if self.attributes.is_object() {
            Ok(())
        } else {
            Err(Error::InvalidServiceOptions(
                "state attributes must be a JSON object".to_string(),
            ))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SetStateResult {
    Created(EntityState),
    Updated(EntityState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeleteStateResult {
    Deleted,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateChangedEvent {
    pub entity_id: EntityId,
    pub old_state: Option<EntityState>,
    pub new_state: Option<EntityState>,
}

#[derive(Clone, Debug)]
pub struct StateWait<'a> {
    ctx: &'a Context,
    entity_id: EntityId,
    target: BinaryState,
    require_transition: bool,
    hold_for: Option<Duration>,
}

impl<'a> StateWait<'a> {
    fn new(ctx: &'a Context, entity_id: EntityId, target: BinaryState) -> Self {
        Self {
            ctx,
            entity_id,
            target,
            require_transition: false,
            hold_for: None,
        }
    }

    pub fn require_transition(mut self) -> Self {
        self.require_transition = true;
        self
    }

    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }

    pub fn within(self, duration: Duration) -> TimedStateWait<'a> {
        TimedStateWait {
            wait: self,
            timeout: duration,
        }
    }
}

impl<'a> IntoFuture for StateWait<'a> {
    type Output = Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_state_wait(self).await.map(|_| ()) })
    }
}

#[derive(Clone, Debug)]
pub struct TimedStateWait<'a> {
    wait: StateWait<'a>,
    timeout: Duration,
}

impl<'a> IntoFuture for TimedStateWait<'a> {
    type Output = Result<WaitResult>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            match self.ctx_timeout().await? {
                TimeoutResult::Completed(()) => Ok(WaitResult::Satisfied),
                TimeoutResult::TimedOut => Ok(WaitResult::TimedOut),
            }
        })
    }
}

impl<'a> TimedStateWait<'a> {
    async fn ctx_timeout(self) -> Result<TimeoutResult<()>> {
        self.wait
            .ctx
            .timeout(self.timeout, run_state_wait(self.wait))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct StateExpectation<'a> {
    ctx: &'a Context,
    entity_id: EntityId,
    target: BinaryState,
    hold_for: Option<Duration>,
}

impl<'a> StateExpectation<'a> {
    fn new(ctx: &'a Context, entity_id: EntityId, target: BinaryState) -> Self {
        Self {
            ctx,
            entity_id,
            target,
            hold_for: None,
        }
    }

    pub fn for_at_least(mut self, duration: Duration) -> Self {
        self.hold_for = Some(duration);
        self
    }
}

impl<'a> IntoFuture for StateExpectation<'a> {
    type Output = Result<HoldResult<BinaryState>>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { run_state_expectation(self).await })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStreamError {
    Lagged { dropped: Option<usize> },
    ConnectionLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    Satisfied,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldResult<T> {
    Held,
    NotSatisfied { actual: T },
    Interrupted { actual: T },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeoutResult<T> {
    Completed(T),
    TimedOut,
}

pub struct TaskHandle<T> {
    inner: BoxFuture<Result<T>>,
}

impl<T: Send + 'static> TaskHandle<T> {
    fn from_join_handle(handle: JoinHandle<Result<T>>) -> Self {
        Self {
            inner: Box::pin(async move {
                handle
                    .await
                    .unwrap_or_else(|error| Err(Error::AutomationTask(error.to_string())))
            }),
        }
    }
}

impl<T> Future for TaskHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

pub struct TimerHandle<T> {
    inner: BoxFuture<Result<T>>,
    control: Arc<TimerControl>,
}

impl<T: Send + 'static> TimerHandle<T> {
    fn from_join_handle(handle: JoinHandle<Result<T>>, control: Arc<TimerControl>) -> Self {
        Self {
            inner: Box::pin(async move {
                handle
                    .await
                    .unwrap_or_else(|error| Err(Error::AutomationTask(error.to_string())))
            }),
            control,
        }
    }

    pub async fn cancel(&mut self) -> Result<()> {
        self.control.cancel();
        self.control.wait_complete().await;
        Ok(())
    }
}

impl<T> Future for TimerHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

#[derive(Debug)]
pub struct StateChangeStream {
    receiver: broadcast::Receiver<StateChangedEvent>,
    entity_filter: Option<EntityId>,
    terminal: bool,
}

impl StateChangeStream {
    fn new(
        receiver: broadcast::Receiver<StateChangedEvent>,
        entity_filter: Option<EntityId>,
    ) -> Self {
        Self {
            receiver,
            entity_filter,
            terminal: false,
        }
    }

    pub async fn next(
        &mut self,
    ) -> Option<std::result::Result<StateChangedEvent, EventStreamError>> {
        if self.terminal {
            return None;
        }

        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if self
                        .entity_filter
                        .as_ref()
                        .is_none_or(|entity_id| event.entity_id == *entity_id)
                    {
                        return Some(Ok(event));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    self.terminal = true;
                    let dropped = usize::try_from(dropped).ok();
                    return Some(Err(EventStreamError::Lagged { dropped }));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    self.terminal = true;
                    return None;
                }
            }
        }
    }
}

#[derive(Debug)]
struct TimerCompletionGuard(Arc<TimerControl>);

impl Drop for TimerCompletionGuard {
    fn drop(&mut self) {
        self.0.complete();
    }
}

#[derive(Debug)]
struct TimerControl {
    is_cancelled: AtomicBool,
    cancelled: watch::Sender<bool>,
    complete: watch::Sender<bool>,
}

impl TimerControl {
    fn new() -> Self {
        let (cancelled, _receiver) = watch::channel(false);
        let (complete, _receiver) = watch::channel(false);
        Self {
            is_cancelled: AtomicBool::new(false),
            cancelled,
            complete,
        }
    }

    fn cancel(&self) {
        if !self.is_cancelled.swap(true, Ordering::AcqRel) {
            let _ = self.cancelled.send(true);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Acquire)
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    fn complete(&self) {
        let _ = self.complete.send(true);
    }

    async fn wait_complete(&self) {
        let mut complete = self.complete.subscribe();
        if *complete.borrow() {
            return;
        }

        while complete.changed().await.is_ok() {
            if *complete.borrow() {
                return;
            }
        }
    }
}

async fn wait_cancelled(cancelled: &mut watch::Receiver<bool>) {
    if *cancelled.borrow() {
        return;
    }

    while cancelled.changed().await.is_ok() {
        if *cancelled.borrow() {
            return;
        }
    }
}

async fn run_state_wait(wait: StateWait<'_>) -> Result<()> {
    wait.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = wait.ctx.state_changes(&wait.entity_id);
    let initial = wait
        .ctx
        .home_assistant
        .generation
        .cached_state(&wait.entity_id);
    let mut ready_for_target = !wait.require_transition
        || initial
            .as_ref()
            .and_then(|state| BinaryState::decode(&state.state).ok())
            != Some(wait.target);

    if !wait.require_transition && is_binary_state(initial.as_ref(), wait.target)? {
        if hold_target_for(
            wait.ctx,
            &mut changes,
            &wait.entity_id,
            wait.target,
            wait.hold_for,
        )
        .await?
        {
            return Ok(());
        }
        ready_for_target = true;
    }

    loop {
        let event = next_state_change(wait.ctx, &mut changes).await?;
        match event.new_state.as_ref() {
            Some(new_state) => {
                let state = BinaryState::decode(&new_state.state)?;
                if state == wait.target {
                    if ready_for_target
                        && hold_target_for(
                            wait.ctx,
                            &mut changes,
                            &wait.entity_id,
                            wait.target,
                            wait.hold_for,
                        )
                        .await?
                    {
                        return Ok(());
                    }
                } else {
                    ready_for_target = true;
                }
            }
            None => {
                return Err(Error::EntityNotFound(wait.entity_id.clone()));
            }
        }
    }
}

async fn run_state_expectation(
    expectation: StateExpectation<'_>,
) -> Result<HoldResult<BinaryState>> {
    expectation.ctx.home_assistant.ensure_generation_active()?;
    let mut changes = expectation.ctx.state_changes(&expectation.entity_id);
    let current = expectation
        .ctx
        .home_assistant
        .generation
        .cached_state(&expectation.entity_id)
        .ok_or_else(|| Error::EntityNotFound(expectation.entity_id.clone()))?;
    let actual = BinaryState::decode(&current.state)?;
    if actual != expectation.target {
        return Ok(HoldResult::NotSatisfied { actual });
    }

    let Some(hold_for) = expectation.hold_for else {
        return Ok(HoldResult::Held);
    };
    if hold_for.is_zero() {
        return Ok(HoldResult::Held);
    }

    let deadline = tokio::time::sleep(hold_for);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => return Ok(HoldResult::Held),
            event = next_state_change(expectation.ctx, &mut changes) => {
                let event = event?;
                match event.new_state.as_ref() {
                    Some(new_state) => {
                        let actual = BinaryState::decode(&new_state.state)?;
                        if actual != expectation.target {
                            return Ok(HoldResult::Interrupted { actual });
                        }
                    }
                    None => return Err(Error::EntityNotFound(expectation.entity_id.clone())),
                }
            }
        }
    }
}

fn is_binary_state(state: Option<&EntityState>, target: BinaryState) -> Result<bool> {
    state
        .map(|state| BinaryState::decode(&state.state).map(|actual| actual == target))
        .transpose()
        .map(|value| value.unwrap_or(false))
}

async fn hold_target_for(
    ctx: &Context,
    changes: &mut StateChangeStream,
    entity_id: &EntityId,
    target: BinaryState,
    hold_for: Option<Duration>,
) -> Result<bool> {
    let Some(hold_for) = hold_for else {
        return Ok(true);
    };
    if hold_for.is_zero() {
        return Ok(true);
    }

    let deadline = tokio::time::sleep(hold_for);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Ok(true),
            event = next_state_change(ctx, changes) => {
                let event = event?;
                match event.new_state.as_ref() {
                    Some(new_state) => {
                        let actual = BinaryState::decode(&new_state.state)?;
                        if actual != target {
                            return Ok(false);
                        }
                    }
                    None => {
                        return Err(Error::EntityNotFound(entity_id.clone()));
                    }
                }
            }
        }
    }
}

async fn next_state_change(
    ctx: &Context,
    changes: &mut StateChangeStream,
) -> Result<StateChangedEvent> {
    tokio::select! {
        event = changes.next() => {
            match event {
                Some(Ok(event)) => Ok(event),
                Some(Err(error)) => Err(Error::EventStream(error)),
                None => Err(Error::Connection("state change stream closed".to_string())),
            }
        }
        () = ctx.cancelled() => Err(Error::Cancelled),
    }
}

#[derive(Debug)]
pub struct RawEventStream {
    _private: (),
}

impl RawEventStream {
    fn placeholder() -> Self {
        Self { _private: () }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightTurnOn {
    pub brightness_pct: Option<u8>,
    pub brightness: Option<u8>,
    pub transition: Option<Duration>,
    pub color_temp_kelvin: Option<u16>,
    pub rgb_color: Option<(u8, u8, u8)>,
    pub effect: Option<String>,
}

impl LightTurnOn {
    pub fn validate(&self) -> Result<()> {
        if let Some(brightness_pct) = self.brightness_pct
            && brightness_pct > 100
        {
            return Err(Error::InvalidServiceOptions(
                "brightness_pct must be in 0..=100".to_string(),
            ));
        }
        Ok(())
    }

    fn into_service_data(self, entity_id: &EntityId) -> Value {
        let mut data = service_entity_map(entity_id);
        insert_some(&mut data, "brightness_pct", self.brightness_pct);
        insert_some(&mut data, "brightness", self.brightness);
        insert_some(&mut data, "color_temp_kelvin", self.color_temp_kelvin);
        insert_some(&mut data, "effect", self.effect);
        if let Some(transition) = self.transition {
            data.insert("transition".to_string(), transition.as_secs_f64().into());
        }
        if let Some((red, green, blue)) = self.rgb_color {
            data.insert(
                "rgb_color".to_string(),
                Value::Array(vec![red.into(), green.into(), blue.into()]),
            );
        }
        Value::Object(data)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightTurnOff {
    pub transition: Option<Duration>,
}

impl LightTurnOff {
    fn into_service_data(self, entity_id: &EntityId) -> Value {
        let mut data = service_entity_map(entity_id);
        if let Some(transition) = self.transition {
            data.insert("transition".to_string(), transition.as_secs_f64().into());
        }
        Value::Object(data)
    }
}

fn service_entity(entity_id: &EntityId) -> Value {
    Value::Object(service_entity_map(entity_id))
}

fn service_entity_map(entity_id: &EntityId) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert("entity_id".to_string(), entity_id.as_str().into());
    data
}

fn insert_some<T>(data: &mut Map<String, Value>, key: &str, value: Option<T>)
where
    T: Into<Value>,
{
    if let Some(value) = value {
        data.insert(key.to_string(), value.into());
    }
}

fn validate_entity_id(value: &str) -> Result<()> {
    let (domain, object_id) = value.split_once('.').ok_or_else(|| {
        invalid_entity_id(
            value,
            "expected `<domain>.<object_id>` with exactly one dot",
        )
    })?;

    if object_id.contains('.') {
        return Err(invalid_entity_id(
            value,
            "expected `<domain>.<object_id>` with exactly one dot",
        ));
    }
    validate_entity_part(value, "domain", domain)?;
    validate_entity_part(value, "object_id", object_id)?;
    Ok(())
}

fn validate_entity_part(full: &str, name: &str, part: &str) -> Result<()> {
    if part.is_empty() {
        return Err(invalid_entity_id(full, format!("{name} must not be empty")));
    }
    if !part
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(invalid_entity_id(
            full,
            format!("{name} may only contain lowercase ASCII letters, digits, and underscores"),
        ));
    }
    Ok(())
}

fn invalid_entity_id(value: &str, reason: impl Into<String>) -> Error {
    Error::InvalidEntityId {
        value: value.to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entity_id_accepts_basic_home_assistant_shape() {
        let id = EntityId::new("binary_sensor.office_occupancy").unwrap();
        assert_eq!(id.domain(), "binary_sensor");
        assert_eq!(id.object_id(), "office_occupancy");
    }

    #[test]
    fn entity_id_rejects_invalid_syntax() {
        for value in [
            "",
            "light",
            ".office",
            "light.",
            "Light.office",
            "light.office-1",
            "light.office.extra",
        ] {
            assert!(EntityId::new(value).is_err(), "{value} should be invalid");
        }
    }

    #[test]
    fn typed_handles_validate_domain() {
        assert!(Light::new("light.office").is_ok());
        assert!(BinarySensor::new("binary_sensor.office_occupancy").is_ok());
        assert!(Switch::new("switch.fan").is_ok());
        assert!(Sensor::<f64>::new("sensor.temperature").is_ok());
        assert!(Light::new("switch.office").is_err());
    }

    #[test]
    fn state_write_requires_object_attributes() {
        assert!(StateWrite::new("ok", json!({ "friendly_name": "Status" })).is_ok());
        assert!(StateWrite::new("bad", json!(["not", "object"])).is_err());
    }

    #[test]
    fn light_turn_on_validates_brightness_pct() {
        assert!(
            LightTurnOn {
                brightness_pct: Some(100),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );

        assert!(
            LightTurnOn {
                brightness_pct: Some(101),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn app_registration_keeps_names() {
        let app = App::new("http://homeassistant.local:8123", "token")
            .automation_fn("noop", |_ctx| async { Ok(()) });

        assert_eq!(app.automation_names(), vec!["noop"]);
    }

    #[test]
    fn app_derives_home_assistant_endpoints() {
        let app = App::new("https://homeassistant.local:8123/", "token");

        assert_eq!(app.home_assistant_url, "https://homeassistant.local:8123");
        assert_eq!(
            app.websocket_url,
            "wss://homeassistant.local:8123/api/websocket"
        );
        assert_eq!(
            app.rest_states_url,
            "https://homeassistant.local:8123/api/states"
        );

        let app = App::new("http://localhost:8123?ignored=true#fragment", "token");
        assert_eq!(app.home_assistant_url, "http://localhost:8123");
        assert_eq!(app.websocket_url, "ws://localhost:8123/api/websocket");
        assert_eq!(app.rest_states_url, "http://localhost:8123/api/states");
    }

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
    fn entity_handle_state_reads_from_cache() {
        run_async(async {
            let light = Light::new("light.office").unwrap();
            let state = sample_state("light.office", "off");
            let ctx = Context::with_seeded_states([state.clone()]);

            assert_eq!(light.state(&ctx).await.unwrap(), state);
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

    #[test]
    fn binary_sensor_wait_satisfies_immediately() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "on")]);

            sensor.wait_until_on(&ctx).await.unwrap();
        });
    }

    #[test]
    fn binary_sensor_wait_require_transition_leaves_and_reenters_target() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "on")]);

            assert_eq!(
                ctx.timeout(
                    Duration::from_millis(5),
                    sensor
                        .wait_until_on(&ctx)
                        .require_transition()
                        .into_future(),
                )
                .await
                .unwrap(),
                TimeoutResult::TimedOut
            );

            let waiter_ctx = ctx.clone();
            let waiter_sensor = sensor.clone();
            let waiter = ctx.spawn(async move {
                waiter_sensor
                    .wait_until_on(&waiter_ctx)
                    .require_transition()
                    .await
            });
            tokio::task::yield_now().await;
            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "off"))
                .unwrap();
            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "on"))
                .unwrap();
            waiter.await.unwrap();
        });
    }

    #[test]
    fn binary_sensor_wait_returns_entity_not_found_when_deleted() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);
            let waiter_ctx = ctx.clone();
            let waiter_sensor = sensor.clone();
            let waiter = ctx.spawn(async move { waiter_sensor.wait_until_on(&waiter_ctx).await });

            tokio::task::yield_now().await;
            ctx.home_assistant()
                .remove_cached_state(sensor.entity_id())
                .unwrap();

            assert!(matches!(
                waiter.await,
                Err(Error::EntityNotFound(entity_id)) if entity_id == *sensor.entity_id()
            ));

            let hold_sensor = BinarySensor::new("binary_sensor.window").unwrap();
            let hold_ctx =
                Context::with_seeded_states([sample_state("binary_sensor.window", "off")]);
            let hold_waiter_ctx = hold_ctx.clone();
            let hold_waiter_sensor = hold_sensor.clone();
            let hold_waiter = hold_ctx.spawn(async move {
                hold_waiter_sensor
                    .wait_until_on(&hold_waiter_ctx)
                    .for_at_least(Duration::from_millis(50))
                    .await
            });

            hold_ctx
                .home_assistant()
                .cache_state(sample_state("binary_sensor.window", "on"))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
            hold_ctx
                .home_assistant()
                .remove_cached_state(hold_sensor.entity_id())
                .unwrap();

            assert!(matches!(
                hold_waiter.await,
                Err(Error::EntityNotFound(entity_id)) if entity_id == *hold_sensor.entity_id()
            ));
        });
    }

    #[test]
    fn binary_sensor_wait_for_at_least_resets_on_other_state() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);
            let waiter_ctx = ctx.clone();
            let waiter_sensor = sensor.clone();
            let mut waiter = ctx.spawn(async move {
                waiter_sensor
                    .wait_until_on(&waiter_ctx)
                    .for_at_least(Duration::from_millis(20))
                    .await
            });

            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "on"))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "off"))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert_eq!(
                ctx.timeout(Duration::from_millis(1), &mut waiter)
                    .await
                    .unwrap(),
                TimeoutResult::TimedOut
            );
            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "on"))
                .unwrap();
            waiter.await.unwrap();
        });
    }

    #[test]
    fn binary_sensor_wait_within_times_out() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);

            assert_eq!(
                sensor
                    .wait_until_on(&ctx)
                    .within(Duration::from_millis(1))
                    .await
                    .unwrap(),
                WaitResult::TimedOut
            );
        });
    }

    #[test]
    fn binary_sensor_expectation_not_satisfied_interrupted_held_and_deleted() {
        run_async(async {
            let sensor = BinarySensor::new("binary_sensor.door").unwrap();
            let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);

            assert_eq!(
                sensor.expect_on(&ctx).await.unwrap(),
                HoldResult::NotSatisfied {
                    actual: BinaryState::Off
                }
            );

            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "on"))
                .unwrap();
            assert_eq!(
                sensor
                    .expect_on(&ctx)
                    .for_at_least(Duration::from_millis(1))
                    .await
                    .unwrap(),
                HoldResult::Held
            );

            let interrupted_ctx = ctx.clone();
            let interrupted_sensor = sensor.clone();
            let interrupted = ctx.spawn(async move {
                interrupted_sensor
                    .expect_on(&interrupted_ctx)
                    .for_at_least(Duration::from_millis(50))
                    .await
            });
            tokio::time::sleep(Duration::from_millis(1)).await;
            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "off"))
                .unwrap();
            assert_eq!(
                interrupted.await.unwrap(),
                HoldResult::Interrupted {
                    actual: BinaryState::Off
                }
            );

            ctx.home_assistant()
                .cache_state(sample_state("binary_sensor.door", "on"))
                .unwrap();
            let deleted_ctx = ctx.clone();
            let deleted_sensor = sensor.clone();
            let deleted = ctx.spawn(async move {
                deleted_sensor
                    .expect_on(&deleted_ctx)
                    .for_at_least(Duration::from_millis(50))
                    .await
            });
            tokio::time::sleep(Duration::from_millis(1)).await;
            ctx.home_assistant()
                .remove_cached_state(sensor.entity_id())
                .unwrap();
            assert!(matches!(
                deleted.await,
                Err(Error::EntityNotFound(entity_id)) if entity_id == *sensor.entity_id()
            ));
        });
    }

    fn run_async(future: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future);
    }

    fn sample_state(entity_id: &str, state: &str) -> EntityState {
        EntityState {
            entity_id: EntityId::new(entity_id).unwrap(),
            state: state.to_string(),
            attributes: Map::new(),
            last_changed: "2026-06-30T00:00:00Z".to_string(),
            last_updated: "2026-06-30T00:00:00Z".to_string(),
        }
    }
}
