use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{EntityId, Error, Result};

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
pub enum SensorValue<T> {
    Value(T),
    Unknown,
    Unavailable,
}

impl<T> SensorValue<T> {
    pub fn as_value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Unavailable => None,
        }
    }

    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Unavailable => None,
        }
    }

    pub fn is_value(&self) -> bool {
        matches!(self, Self::Value(_))
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
