//! A small Rust automation framework scaffold for Home Assistant.
//!
//! This crate currently defines the public surface proposed for `hauto`.
//! Runtime, transport, cache, and event fan-out behavior are intentionally
//! placeholder implementations for now.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fmt,
    future::{Future, IntoFuture},
    marker::PhantomData,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use thiserror::Error as ThisError;

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
            .field("registrations", &self.automation_names())
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            home_assistant_url: url.into(),
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
        let registrations = self.registrations;
        for registration in &registrations {
            let _ = &registration.run;
        }
        let _ = (self.access_token, registrations);
        Err(Error::NotImplemented("App::run"))
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
    pub fn home_assistant(&self) -> HomeAssistantClient {
        self.home_assistant.clone()
    }

    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + 'static {
        std::future::pending()
    }

    pub fn spawn<F, T>(&self, _future: F) -> TaskHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        TaskHandle::placeholder("Context::spawn")
    }

    pub async fn sleep(&self, _duration: Duration) -> Result<()> {
        Err(Error::NotImplemented("Context::sleep"))
    }

    pub async fn timeout<F, T>(&self, _duration: Duration, _future: F) -> Result<TimeoutResult<T>>
    where
        F: Future<Output = Result<T>> + Send,
        T: Send,
    {
        Err(Error::NotImplemented("Context::timeout"))
    }

    pub fn run_after<F, T>(&self, _duration: Duration, _future: F) -> TimerHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        TimerHandle::placeholder("Context::run_after")
    }

    pub fn state_changes(&self, _entity: &EntityId) -> StateChangeStream {
        StateChangeStream::placeholder()
    }

    pub fn binary_sensor_changes(&self, sensor: &BinarySensor) -> StateChangeStream {
        self.state_changes(sensor.entity_id())
    }

    pub fn light_changes(&self, light: &Light) -> StateChangeStream {
        self.state_changes(light.entity_id())
    }
}

#[derive(Clone, Debug, Default)]
pub struct HomeAssistantClient;

impl HomeAssistantClient {
    pub async fn call_service_raw(
        &self,
        _domain: &str,
        _service: &str,
        _data: Value,
    ) -> Result<Value> {
        Err(Error::NotImplemented(
            "HomeAssistantClient::call_service_raw",
        ))
    }

    pub async fn command_raw(&self, command: Value) -> Result<Value> {
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
        Err(Error::NotImplemented("HomeAssistantClient::set_state_raw"))
    }

    pub async fn delete_state_raw(&self, _entity_id: &EntityId) -> Result<DeleteStateResult> {
        Err(Error::NotImplemented(
            "HomeAssistantClient::delete_state_raw",
        ))
    }

    pub async fn get_state_raw(&self, entity_id: &EntityId) -> Result<EntityState> {
        Err(Error::EntityNotFound(entity_id.clone()))
    }

    pub async fn subscribe_state_changes(&self) -> Result<StateChangeStream> {
        Ok(StateChangeStream::placeholder())
    }

    pub async fn subscribe_events_raw(&self, _event_type: Option<&str>) -> Result<RawEventStream> {
        Ok(RawEventStream::placeholder())
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
        Box::pin(async move {
            let _ = (
                self.ctx,
                self.entity_id,
                self.target,
                self.require_transition,
                self.hold_for,
            );
            Err(Error::NotImplemented("StateWait"))
        })
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
            let _ = (self.wait, self.timeout);
            Err(Error::NotImplemented("TimedStateWait"))
        })
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
        Box::pin(async move {
            let _ = (self.ctx, self.entity_id, self.target, self.hold_for);
            Err(Error::NotImplemented("StateExpectation"))
        })
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
    fn placeholder(name: &'static str) -> Self {
        Self {
            inner: Box::pin(async move { Err(Error::NotImplemented(name)) }),
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
}

impl<T: Send + 'static> TimerHandle<T> {
    fn placeholder(name: &'static str) -> Self {
        Self {
            inner: Box::pin(async move { Err(Error::NotImplemented(name)) }),
        }
    }

    pub async fn cancel(&mut self) -> Result<()> {
        Err(Error::NotImplemented("TimerHandle::cancel"))
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
    _private: (),
}

impl StateChangeStream {
    fn placeholder() -> Self {
        Self { _private: () }
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
}
