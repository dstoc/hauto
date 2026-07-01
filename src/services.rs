use std::time::Duration;

use serde_json::{Map, Value};

use crate::{Error, Result, entity::EntityId};

/// Options for a Home Assistant `light.turn_on` service call.
///
/// Every `None` field is omitted from the service data, allowing Home
/// Assistant to retain or choose that setting. `brightness_pct` and
/// `brightness` are alternative brightness representations; callers should
/// normally set at most one. If both are set, both are forwarded and Home
/// Assistant decides how to handle the combination.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightTurnOn {
    /// Brightness as a percentage in the inclusive range `0..=100`.
    pub brightness_pct: Option<u8>,
    /// Brightness in Home Assistant's inclusive `0..=255` scale.
    pub brightness: Option<u8>,
    /// Fade duration, encoded in the service data as fractional seconds.
    pub transition: Option<Duration>,
    /// Color temperature in kelvin.
    ///
    /// No range is enforced locally because supported ranges vary by light.
    pub color_temp_kelvin: Option<u16>,
    /// Red, green, and blue channels, each in the inclusive range `0..=255`.
    pub rgb_color: Option<(u8, u8, u8)>,
    /// Integration-defined effect name.
    pub effect: Option<String>,
}

impl LightTurnOn {
    /// Validates locally enforceable option ranges.
    ///
    /// Returns an error when `brightness_pct` exceeds `100`. Device-specific
    /// color-temperature and effect support is left to Home Assistant.
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

    pub(crate) fn into_service_data(self, entity_id: &EntityId) -> Value {
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

/// Options for a Home Assistant `light.turn_off` service call.
///
/// The call always targets the handle's entity ID. A missing transition is
/// omitted from the service data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightTurnOff {
    /// Fade duration, encoded in the service data as fractional seconds.
    pub transition: Option<Duration>,
}

impl LightTurnOff {
    pub(crate) fn into_service_data(self, entity_id: &EntityId) -> Value {
        let mut data = service_entity_map(entity_id);
        if let Some(transition) = self.transition {
            data.insert("transition".to_string(), transition.as_secs_f64().into());
        }
        Value::Object(data)
    }
}

pub(crate) fn service_entity(entity_id: &EntityId) -> Value {
    Value::Object(service_entity_map(entity_id))
}

pub(crate) fn service_entity_map(entity_id: &EntityId) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert("entity_id".to_string(), entity_id.as_str().into());
    data
}

pub(crate) fn insert_some<T>(data: &mut Map<String, Value>, key: &str, value: Option<T>)
where
    T: Into<Value>,
{
    if let Some(value) = value {
        data.insert(key.to_string(), value.into());
    }
}

pub(crate) fn validate_domain_service(domain: &str, service: &str) -> Result<()> {
    if domain.trim().is_empty() {
        return Err(Error::InvalidServiceOptions(
            "service domain must not be empty".to_string(),
        ));
    }
    if service.trim().is_empty() {
        return Err(Error::InvalidServiceOptions(
            "service name must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
