use std::time::Duration;

use serde_json::{Map, Value};

use crate::{EntityId, Error, Result};

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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightTurnOff {
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
