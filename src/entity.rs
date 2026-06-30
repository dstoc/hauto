use std::{fmt, marker::PhantomData, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BinaryState, Context, EntityState, Error, LightTurnOff, LightTurnOn, Result, SensorValue,
    StateCache, StateExpectation, StateWait, service_entity,
};

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
            pub fn read(&self, cache: &StateCache<'_>) -> Result<Option<BinaryState>> {
                self.read_typed(cache)
            }

            pub fn wait_until<'a>(
                &'a self,
                ctx: &'a Context,
                target: BinaryState,
            ) -> StateWait<'a> {
                self.wait_until_typed(ctx, target)
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
                self.expect_typed(ctx, target)
            }

            pub fn expect_on<'a>(&'a self, ctx: &'a Context) -> StateExpectation<'a> {
                self.expect_state(ctx, BinaryState::On)
            }

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
            pub fn read(&self, cache: &StateCache<'_>) -> Result<Option<$state>> {
                self.read_typed(cache)
            }

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
