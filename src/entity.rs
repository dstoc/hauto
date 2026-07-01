use std::{fmt, marker::PhantomData, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::state::{BinaryState, SensorValue};

use crate::{
    Error, Result,
    context::Context,
    service::{LightTurnOff, LightTurnOn},
    service_entity,
    state::{EntityState, StateCache},
    wait::{StateExpectation, StateWait},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
/// A syntactically validated Home Assistant entity ID.
///
/// Validation requires exactly `<domain>.<object_id>` using lowercase ASCII
/// letters, digits, and underscores. Construction does not contact Home
/// Assistant and therefore does not establish that the entity exists.
pub struct EntityId(String);

impl EntityId {
    /// Validates and constructs an entity ID without checking its existence.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_entity_id(&value)?;
        Ok(Self(value))
    }

    /// Returns the complete `<domain>.<object_id>` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the portion before the dot.
    pub fn domain(&self) -> &str {
        self.0
            .split_once('.')
            .expect("EntityId invariant guarantees one dot")
            .0
    }

    /// Returns the portion after the dot.
    pub fn object_id(&self) -> &str {
        self.0
            .split_once('.')
            .expect("EntityId invariant guarantees one dot")
            .1
    }

    /// Verifies that this ID belongs to `expected`.
    ///
    /// Returns `InvalidDomain` on a mismatch and does not contact Home
    /// Assistant.
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

/// A typed handle for an entity in Home Assistant's `light` domain.
///
/// The handle stores identity only; construction does not check existence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Light {
    entity_id: EntityId,
}

/// A typed handle for an entity in Home Assistant's `switch` domain.
///
/// The handle stores identity only; construction does not check existence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    entity_id: EntityId,
}

/// A typed handle for an entity in Home Assistant's `binary_sensor` domain.
///
/// The handle stores identity only; construction does not check existence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinarySensor {
    entity_id: EntityId,
}

/// A typed handle for a Home Assistant `sensor` and its decoding policy.
///
/// `Sensor<f64>` strictly requires a numeric state.
/// `Sensor<SensorValue<f64>>` preserves `unknown` and `unavailable`, while
/// `Sensor<String>` returns the raw state string. Construction does not check
/// existence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sensor<T> {
    entity_id: EntityId,
    _state: PhantomData<T>,
}

macro_rules! entity_handle {
    ($ty:ty, $domain:literal) => {
        impl $ty {
            /// Constructs a handle after validating entity-ID syntax and domain.
            ///
            /// This does not contact Home Assistant or check that the entity
            /// currently exists.
            pub fn new(entity_id: impl Into<String>) -> Result<Self> {
                let entity_id = EntityId::new(entity_id)?;
                entity_id.ensure_domain($domain)?;
                Ok(Self { entity_id })
            }

            /// Returns the validated entity ID stored by this handle.
            pub fn entity_id(&self) -> &EntityId {
                &self.entity_id
            }

            /// Reads this entity's raw current state through the Home Assistant client.
            ///
            /// Unlike `read`, this is asynchronous and returns `EntityNotFound`
            /// for a missing entity. It does not decode the state into the
            /// handle's typed representation.
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
    /// Constructs a sensor handle after validating entity-ID syntax and domain.
    ///
    /// This does not contact Home Assistant or check that the entity currently
    /// exists.
    pub fn new(entity_id: impl Into<String>) -> Result<Self> {
        let entity_id = EntityId::new(entity_id)?;
        entity_id.ensure_domain("sensor")?;
        Ok(Self {
            entity_id,
            _state: PhantomData,
        })
    }

    /// Returns the validated entity ID stored by this handle.
    pub fn entity_id(&self) -> &EntityId {
        &self.entity_id
    }

    /// Reads this sensor's raw current state through the Home Assistant client.
    ///
    /// Unlike `read`, this is asynchronous and returns `EntityNotFound` for a
    /// missing entity. It does not apply `T`'s decoding policy.
    pub async fn state(&self, ctx: &Context) -> Result<EntityState> {
        ctx.home_assistant().get_state_raw(&self.entity_id).await
    }
}

impl Light {
    /// Calls Home Assistant's `light.turn_on` service for this entity.
    ///
    /// Options are validated before a service payload containing `entity_id`
    /// and all non-omitted fields is sent. Connection-generation cancellation
    /// is reported as an error.
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

    /// Calls Home Assistant's `light.turn_off` service for this entity.
    ///
    /// The payload contains `entity_id` and the transition when supplied.
    /// Connection-generation cancellation is reported as an error.
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
    /// Calls Home Assistant's `switch.turn_on` service for this entity.
    ///
    /// The raw Home Assistant response is returned. Connection-generation
    /// cancellation is reported as an error.
    pub async fn turn_on(&self, ctx: &Context) -> Result<Value> {
        ctx.home_assistant()
            .call_service_raw("switch", "turn_on", service_entity(&self.entity_id))
            .await
    }

    /// Calls Home Assistant's `switch.turn_off` service for this entity.
    ///
    /// The raw Home Assistant response is returned. Connection-generation
    /// cancellation is reported as an error.
    pub async fn turn_off(&self, ctx: &Context) -> Result<Value> {
        ctx.home_assistant()
            .call_service_raw("switch", "turn_off", service_entity(&self.entity_id))
            .await
    }
}

pub(crate) trait StateDecoder<T> {
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<T>;
}

pub(crate) struct BinaryStateDecoder;
pub(crate) struct F64StateDecoder;
pub(crate) struct SensorValueF64Decoder;
pub(crate) struct StringStateDecoder;

impl StateDecoder<BinaryState> for BinaryStateDecoder {
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<BinaryState> {
        BinaryState::decode(&raw.state).map_err(|error| Error::InvalidState {
            entity_id: entity_id.clone(),
            reason: error.to_string(),
        })
    }
}

impl StateDecoder<f64> for F64StateDecoder {
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<f64> {
        raw.state
            .parse::<f64>()
            .map_err(|error| Error::InvalidState {
                entity_id: entity_id.clone(),
                reason: format!("expected numeric state, got `{}`: {error}", raw.state),
            })
    }
}

impl StateDecoder<SensorValue<f64>> for SensorValueF64Decoder {
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<SensorValue<f64>> {
        match raw.state.as_str() {
            "" | "unknown" => Ok(SensorValue::Unknown),
            "unavailable" => Ok(SensorValue::Unavailable),
            state => state
                .parse::<f64>()
                .map(SensorValue::Value)
                .map_err(|error| Error::InvalidState {
                    entity_id: entity_id.clone(),
                    reason: format!("expected numeric state, got `{}`: {error}", raw.state),
                }),
        }
    }
}

impl StateDecoder<String> for StringStateDecoder {
    fn decode_state(_entity_id: &EntityId, raw: &EntityState) -> Result<String> {
        Ok(raw.state.clone())
    }
}

pub(crate) trait TypedReadableEntity {
    type State: Clone + Send + Sync + 'static;
    type Decoder: StateDecoder<Self::State>;

    fn entity_id(&self) -> &EntityId;
}

trait TypedReadableEntityExt: TypedReadableEntity {
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<Self::State> {
        <Self::Decoder as StateDecoder<Self::State>>::decode_state(entity_id, raw)
    }

    fn read_typed(&self, cache: &StateCache<'_>) -> Result<Option<Self::State>> {
        let entity_id = self.entity_id();
        cache
            .raw_state(entity_id)
            .map(|raw| Self::decode_state(entity_id, &raw))
            .transpose()
    }

    async fn get_typed(&self, ctx: &Context) -> Result<Self::State> {
        let entity_id = self.entity_id();
        let raw = ctx.home_assistant().get_state_raw(entity_id).await?;
        Self::decode_state(entity_id, &raw)
    }

    async fn next_change_typed(&self, ctx: &Context) -> Result<Self::State> {
        let entity_id = self.entity_id();
        let mut changes = ctx.state_changes(entity_id);

        let event = tokio::select! {
            event = changes.next() => {
                event
                    .ok_or_else(|| Error::Connection("state change stream closed".to_string()))?
                    .map_err(Error::EventStream)?
            }
            () = ctx.cancelled() => return Err(Error::Cancelled),
        };

        let raw = event
            .new_state
            .ok_or_else(|| Error::EntityNotFound(entity_id.clone()))?;
        Self::decode_state(entity_id, &raw)
    }

    fn wait_until_matching_typed<'a, F>(
        &'a self,
        ctx: &'a Context,
        predicate: F,
    ) -> StateWait<'a, Self::State>
    where
        F: Fn(&Self::State) -> bool + Send + Sync + 'static,
    {
        StateWait::matching(ctx, self.entity_id().clone(), Self::decode_state, predicate)
    }

    fn wait_until_typed<'a>(
        &'a self,
        ctx: &'a Context,
        target: Self::State,
    ) -> StateWait<'a, Self::State>
    where
        Self::State: PartialEq,
    {
        StateWait::new(ctx, self.entity_id().clone(), Self::decode_state, target)
    }

    fn expect_matching_typed<'a, F>(
        &'a self,
        ctx: &'a Context,
        predicate: F,
    ) -> StateExpectation<'a, Self::State>
    where
        F: Fn(&Self::State) -> bool + Send + Sync + 'static,
    {
        StateExpectation::matching(ctx, self.entity_id().clone(), Self::decode_state, predicate)
    }

    fn expect_typed<'a>(
        &'a self,
        ctx: &'a Context,
        target: Self::State,
    ) -> StateExpectation<'a, Self::State>
    where
        Self::State: PartialEq,
    {
        StateExpectation::new(ctx, self.entity_id().clone(), Self::decode_state, target)
    }
}

impl<E> TypedReadableEntityExt for E where E: TypedReadableEntity {}

macro_rules! typed_readable_entity {
    ($ty:ty, $state:ty, $decoder:ty) => {
        impl TypedReadableEntity for $ty {
            type State = $state;
            type Decoder = $decoder;

            fn entity_id(&self) -> &EntityId {
                &self.entity_id
            }
        }
    };
}

typed_readable_entity!(BinarySensor, BinaryState, BinaryStateDecoder);
typed_readable_entity!(Light, BinaryState, BinaryStateDecoder);
typed_readable_entity!(Switch, BinaryState, BinaryStateDecoder);
typed_readable_entity!(Sensor<f64>, f64, F64StateDecoder);
typed_readable_entity!(
    Sensor<SensorValue<f64>>,
    SensorValue<f64>,
    SensorValueF64Decoder
);
typed_readable_entity!(Sensor<String>, String, StringStateDecoder);

macro_rules! binary_state_entity {
    ($ty:ty) => {
        impl $ty {
            /// Decodes this entity from the current connection generation's cache.
            ///
            /// Returns `Ok(None)` only when the entity is missing. Explicit
            /// `unknown` and `unavailable` states are returned as `BinaryState`
            /// variants; any other state string is an error.
            pub fn read(&self, cache: &StateCache<'_>) -> Result<Option<BinaryState>> {
                self.read_typed(cache)
            }

            /// Fetches and decodes this entity's current state.
            ///
            /// A missing entity, invalid binary state, or cancelled connection
            /// generation is returned as an error.
            pub async fn get(&self, ctx: &Context) -> Result<BinaryState> {
                self.get_typed(ctx).await
            }

            /// Waits for and decodes the next state-change event for this entity.
            ///
            /// This ignores the current cached state. Entity deletion is reported
            /// as `EntityNotFound`; stream failure, invalid state, and connection
            /// generation cancellation are returned as errors. A reconnect starts
            /// a new automation generation rather than resuming this future.
            pub async fn next_change(&self, ctx: &Context) -> Result<BinaryState> {
                self.next_change_typed(ctx).await
            }

            /// Builds a wait for this entity to equal `target`.
            ///
            /// By default an already-matching cached state satisfies the wait.
            /// Builder options can require a later transition or continuous
            /// satisfaction. Connection loss cancels the wait and any held
            /// duration is not resumed after reconnection.
            pub fn wait_until<'a>(
                &'a self,
                ctx: &'a Context,
                target: BinaryState,
            ) -> StateWait<'a> {
                self.wait_until_typed(ctx, target)
            }

            /// Builds a wait for the `On` state.
            ///
            /// An already-on cached state satisfies the wait unless
            /// `require_transition` is selected on the returned builder.
            pub fn wait_until_on<'a>(&'a self, ctx: &'a Context) -> StateWait<'a> {
                self.wait_until(ctx, BinaryState::On)
            }

            /// Builds a wait for the `Off` state.
            ///
            /// An already-off cached state satisfies the wait unless
            /// `require_transition` is selected on the returned builder.
            pub fn wait_until_off<'a>(&'a self, ctx: &'a Context) -> StateWait<'a> {
                self.wait_until(ctx, BinaryState::Off)
            }

            /// Builds an immediate expectation that this entity equals `target`.
            ///
            /// The current cached state is checked when the expectation runs.
            /// A missing entity or invalid state is an error. Holding the
            /// expectation across connection loss is cancelled and is not resumed.
            pub fn expect_state<'a>(
                &'a self,
                ctx: &'a Context,
                target: BinaryState,
            ) -> StateExpectation<'a> {
                self.expect_typed(ctx, target)
            }

            /// Builds an immediate expectation that this entity is on.
            pub fn expect_on<'a>(&'a self, ctx: &'a Context) -> StateExpectation<'a> {
                self.expect_state(ctx, BinaryState::On)
            }

            /// Builds an immediate expectation that this entity is off.
            pub fn expect_off<'a>(&'a self, ctx: &'a Context) -> StateExpectation<'a> {
                self.expect_state(ctx, BinaryState::Off)
            }
        }
    };
}

binary_state_entity!(BinarySensor);
binary_state_entity!(Light);
binary_state_entity!(Switch);

macro_rules! sensor_state_entity {
    ($state:ty) => {
        impl Sensor<$state> {
            /// Decodes this sensor from the current connection generation's cache.
            ///
            /// Returns `Ok(None)` when the entity is missing. Otherwise the
            /// sensor's selected decoding policy is applied and decoding failures
            /// are returned as errors.
            pub fn read(&self, cache: &StateCache<'_>) -> Result<Option<$state>> {
                self.read_typed(cache)
            }

            /// Fetches and decodes this sensor's current state.
            ///
            /// A missing entity, decoding failure, or cancelled connection
            /// generation is returned as an error. `Sensor<f64>` is strict;
            /// `Sensor<SensorValue<f64>>` preserves explicit availability states.
            pub async fn get(&self, ctx: &Context) -> Result<$state> {
                self.get_typed(ctx).await
            }

            /// Waits for and decodes the next state-change event for this sensor.
            ///
            /// This ignores the current cached state. Entity deletion is reported
            /// as `EntityNotFound`; stream failure, decoding failure, and
            /// connection cancellation are errors. Reconnection does not resume
            /// this future in the next automation generation.
            pub async fn next_change(&self, ctx: &Context) -> Result<$state> {
                self.next_change_typed(ctx).await
            }

            /// Builds a wait until a decoded sensor value satisfies `predicate`.
            ///
            /// The cached value is tested first, so an initially matching value
            /// satisfies the wait unless the returned builder is configured to
            /// require a transition. Connection loss cancels the wait and any
            /// held duration is not resumed after reconnection.
            pub fn wait_until_matching<'a, F>(
                &'a self,
                ctx: &'a Context,
                predicate: F,
            ) -> StateWait<'a, $state>
            where
                F: Fn(&$state) -> bool + Send + Sync + 'static,
            {
                self.wait_until_matching_typed(ctx, predicate)
            }

            /// Builds an immediate expectation over this sensor's decoded value.
            ///
            /// The current cached value is checked when the expectation runs.
            /// Missing entities and decoding failures are errors. A continuous
            /// hold is cancelled, rather than resumed, across reconnection.
            pub fn expect_matching<'a, F>(
                &'a self,
                ctx: &'a Context,
                predicate: F,
            ) -> StateExpectation<'a, $state>
            where
                F: Fn(&$state) -> bool + Send + Sync + 'static,
            {
                self.expect_matching_typed(ctx, predicate)
            }
        }
    };
}

sensor_state_entity!(f64);
sensor_state_entity!(SensorValue<f64>);
sensor_state_entity!(String);

pub(crate) fn validate_entity_id(value: &str) -> Result<()> {
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
mod tests;
