use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::cache::StateCache;

use crate::{Error, Result, entity::EntityId};

/// A decoded Home Assistant binary state.
///
/// Unlike a missing entity, `Unknown` and `Unavailable` represent entities
/// that exist and reported those explicit state strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryState {
    /// The Home Assistant state string is `"on"`.
    On,
    /// The Home Assistant state string is `"off"`.
    Off,
    /// Home Assistant knows the entity but not its current value.
    Unknown,
    /// Home Assistant knows the entity but cannot currently obtain its value.
    Unavailable,
}

impl BinaryState {
    /// Decodes Home Assistant's binary state strings.
    ///
    /// Returns an error for every string other than `on`, `off`, `unknown`,
    /// and `unavailable`.
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

/// A sensor reading that preserves Home Assistant availability states.
///
/// Use `Sensor<SensorValue<f64>>` when `unknown` and `unavailable` should be
/// handled as data. `Sensor<f64>` instead uses strict decoding and returns an
/// error for either string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SensorValue<T> {
    /// A successfully decoded sensor value.
    Value(T),
    /// The entity exists, but Home Assistant does not know its current value.
    Unknown,
    /// The entity exists, but its integration cannot currently provide a value.
    Unavailable,
}

impl<T> SensorValue<T> {
    /// Borrows the decoded value, returning `None` for either availability state.
    pub fn as_value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Unavailable => None,
        }
    }

    /// Extracts the decoded value, returning `None` for either availability state.
    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Unavailable => None,
        }
    }

    /// Returns whether this contains a decoded value.
    pub fn is_value(&self) -> bool {
        matches!(self, Self::Value(_))
    }

    /// Returns whether Home Assistant reported `unknown`.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// Returns whether Home Assistant reported `unavailable`.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// Home Assistant's raw representation of one state-machine entry.
///
/// This may describe an entity-registry-backed entity or an ephemeral entry
/// created through the REST state API. Presence here does not imply an entity
/// registry entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    /// The state-machine key in `<domain>.<object_id>` form.
    pub entity_id: EntityId,
    /// The raw state string, including values such as `unknown` or `unavailable`.
    pub state: String,
    /// Arbitrary state attributes supplied by Home Assistant.
    #[serde(default)]
    pub attributes: Map<String, Value>,
    /// Home Assistant's timestamp for when the state value last changed.
    pub last_changed: String,
    /// Home Assistant's timestamp for when the state or its attributes last changed.
    pub last_updated: String,
}

/// The state and attributes sent to Home Assistant's REST state API.
///
/// Publishing creates or updates a state-machine entry; it does not create an
/// entity-registry entry or control a physical device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateWrite {
    /// The raw state string to publish.
    pub state: String,
    /// The complete JSON object of attributes to publish.
    pub attributes: Value,
}

impl StateWrite {
    /// Creates a validated REST state payload.
    ///
    /// Returns an error when `attributes` is not a JSON object.
    pub fn new(state: impl Into<String>, attributes: Value) -> Result<Self> {
        let write = Self {
            state: state.into(),
            attributes,
        };
        write.validate()?;
        Ok(write)
    }

    /// Checks that `attributes` is a JSON object.
    ///
    /// This is also called by the raw state-publishing operation, so directly
    /// deserialized or struct-literal values are validated before transmission.
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

/// Whether a REST state publication created or replaced a state-machine entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SetStateResult {
    /// Home Assistant created a new, possibly ephemeral, state-machine entry.
    Created(EntityState),
    /// Home Assistant replaced the state of an existing state-machine entry.
    Updated(EntityState),
}

/// The outcome of deleting a state-machine entry through the REST API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeleteStateResult {
    /// Home Assistant deleted the state-machine entry.
    Deleted,
    /// No state-machine entry existed for the requested entity ID.
    NotFound,
}

/// A decoded Home Assistant `state_changed` event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateChangedEvent {
    /// The entity whose state-machine entry changed.
    pub entity_id: EntityId,
    /// The state before the event, or `None` when the entry was created.
    pub old_state: Option<EntityState>,
    /// The state after the event, or `None` when the entry was deleted.
    pub new_state: Option<EntityState>,
}
