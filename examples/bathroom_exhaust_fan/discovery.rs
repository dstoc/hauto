//! Example-specific entity discovery and override handling.

use hauto::{
    BinarySensor, Context, EntityCatalog, EntityId, EntitySet, Sensor, SensorValue, Switch,
};

use crate::{
    fan_control::{FanBathroomConfig, FanControlConfig, FanSettings},
    humidity_status::{HumiditySettings, HumidityStatusConfig},
};

#[derive(Clone, Debug)]
pub struct BathroomSpec {
    name: String,
    area_name: Option<String>,
    temperature: Option<String>,
    humidity: Option<String>,
    occupancy: Option<String>,
    humidity_status: Option<String>,
}

impl BathroomSpec {
    pub fn new(
        name: impl Into<String>,
        area_name: Option<String>,
        temperature: Option<String>,
        humidity: Option<String>,
        occupancy: Option<String>,
        humidity_status: Option<String>,
        area_variable: &str,
    ) -> Result<Self, String> {
        if area_name.is_none()
            && (temperature.is_none()
                || humidity.is_none()
                || occupancy.is_none()
                || humidity_status.is_none())
        {
            return Err(missing(area_variable));
        }

        Ok(Self {
            name: name.into(),
            area_name,
            temperature,
            humidity,
            occupancy,
            humidity_status,
        })
    }

    fn needs_area_for_humidity(&self) -> bool {
        self.temperature.is_none() || self.humidity.is_none() || self.humidity_status.is_none()
    }

    fn needs_area_for_fan(&self) -> bool {
        self.occupancy.is_none() || self.humidity_status.is_none()
    }

    pub fn display_name(&self) -> &str {
        self.area_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct AmbientSpec {
    area_name: Option<String>,
    temperature: Option<String>,
    humidity: Option<String>,
}

impl AmbientSpec {
    pub fn new(
        area_name: Option<String>,
        temperature: Option<String>,
        humidity: Option<String>,
    ) -> Result<Self, String> {
        if area_name.is_none() && (temperature.is_none() || humidity.is_none()) {
            return Err(missing("HAUTO_AMBIENT_AREA"));
        }

        Ok(Self {
            area_name,
            temperature,
            humidity,
        })
    }

    fn needs_area(&self) -> bool {
        self.temperature.is_none() || self.humidity.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct FanSpec {
    name: Option<String>,
    entity_id: Option<String>,
}

impl FanSpec {
    pub fn new(name: Option<String>, entity_id: Option<String>) -> Result<Self, String> {
        if name.is_none() && entity_id.is_none() {
            return Err(missing("HAUTO_EXHAUST_FAN_NAME"));
        }
        Ok(Self { name, entity_id })
    }
}

pub async fn resolve_humidity_config(
    ctx: &Context,
    bathroom: &BathroomSpec,
    ambient: &AmbientSpec,
    settings: HumiditySettings,
) -> hauto::Result<HumidityStatusConfig> {
    let catalog = if bathroom.needs_area_for_humidity() || ambient.needs_area() {
        Some(ctx.entity_catalog().await?)
    } else {
        None
    };

    let bathroom_area = if bathroom.needs_area_for_humidity() {
        Some(area(&catalog, bathroom.area_name.as_deref())?)
    } else {
        None
    };
    let bathroom_entities = if bathroom.temperature.is_none() || bathroom.humidity.is_none() {
        Some(
            catalog
                .as_ref()
                .expect("catalog loaded when bathroom discovery is needed")
                .entities_in(
                    bathroom_area
                        .as_ref()
                        .expect("area resolved when bathroom discovery is needed"),
                )
                .await?,
        )
    } else {
        None
    };

    let ambient_area = if ambient.needs_area() {
        Some(area(&catalog, ambient.area_name.as_deref())?)
    } else {
        None
    };
    let ambient_entities = if ambient.needs_area() {
        Some(
            catalog
                .as_ref()
                .expect("catalog loaded when ambient discovery is needed")
                .entities_in(
                    ambient_area
                        .as_ref()
                        .expect("area resolved when ambient discovery is needed"),
                )
                .await?,
        )
    } else {
        None
    };

    Ok(HumidityStatusConfig {
        name: bathroom.display_name().to_string(),
        status_entity: status_entity(bathroom, bathroom_area.as_ref())?,
        bathroom_temperature: temperature_sensor(
            bathroom.temperature.as_deref(),
            bathroom_entities.as_ref(),
        )?,
        bathroom_humidity: humidity_sensor(
            bathroom.humidity.as_deref(),
            bathroom_entities.as_ref(),
        )?,
        ambient_temperature: temperature_sensor(
            ambient.temperature.as_deref(),
            ambient_entities.as_ref(),
        )?,
        ambient_humidity: humidity_sensor(ambient.humidity.as_deref(), ambient_entities.as_ref())?,
        settings,
    })
}

pub async fn resolve_fan_config(
    ctx: &Context,
    bathrooms: &[BathroomSpec; 2],
    fan: &FanSpec,
    settings: FanSettings,
) -> hauto::Result<FanControlConfig> {
    let needs_catalog =
        fan.entity_id.is_none() || bathrooms.iter().any(BathroomSpec::needs_area_for_fan);
    let catalog = if needs_catalog {
        Some(ctx.entity_catalog().await?)
    } else {
        None
    };

    let bathroom_1 = resolve_fan_bathroom(&catalog, &bathrooms[0]).await?;
    let bathroom_2 = resolve_fan_bathroom(&catalog, &bathrooms[1]).await?;
    let fan = match fan.entity_id.as_deref() {
        Some(entity_id) => Switch::new(entity_id)?,
        None => catalog
            .as_ref()
            .expect("catalog loaded when fan discovery is needed")
            .entities()
            .query()
            .domain("switch")
            .named(
                fan.name
                    .as_deref()
                    .expect("fan name validated when override is absent"),
            )
            .exactly_one()?
            .switch()?,
    };

    Ok(FanControlConfig {
        fan,
        bathrooms: [bathroom_1, bathroom_2],
        settings,
    })
}

async fn resolve_fan_bathroom(
    catalog: &Option<EntityCatalog>,
    bathroom: &BathroomSpec,
) -> hauto::Result<FanBathroomConfig> {
    let area = if bathroom.needs_area_for_fan() {
        Some(area(catalog, bathroom.area_name.as_deref())?)
    } else {
        None
    };
    let entities = if bathroom.occupancy.is_none() {
        Some(
            catalog
                .as_ref()
                .expect("catalog loaded when occupancy discovery is needed")
                .entities_in(
                    area.as_ref()
                        .expect("area resolved when occupancy discovery is needed"),
                )
                .await?,
        )
    } else {
        None
    };

    let occupancy = match bathroom.occupancy.as_deref() {
        Some(entity_id) => BinarySensor::new(entity_id)?,
        None => entities
            .as_ref()
            .expect("entities loaded when occupancy override is absent")
            .query()
            .domain("binary_sensor")
            .device_class_in(["occupancy", "motion"])
            .exactly_one()?
            .binary_sensor()?,
    };
    let humidity_status = Sensor::<String>::new(status_entity(bathroom, area.as_ref())?.as_str())?;

    Ok(FanBathroomConfig {
        name: bathroom.display_name().to_string(),
        humidity_status,
        occupancy,
    })
}

fn area(
    catalog: &Option<EntityCatalog>,
    area_name: Option<&str>,
) -> hauto::Result<hauto::AreaInfo> {
    catalog
        .as_ref()
        .expect("catalog loaded when an area is needed")
        .area_named(area_name.expect("area name validated when discovery is needed"))
}

fn temperature_sensor(
    override_id: Option<&str>,
    entities: Option<&EntitySet>,
) -> hauto::Result<Sensor<SensorValue<f64>>> {
    match override_id {
        Some(entity_id) => Sensor::new(entity_id),
        None => entities
            .expect("entities loaded when temperature override is absent")
            .query()
            .domain("sensor")
            .device_class("temperature")
            .exactly_one()?
            .sensor(),
    }
}

fn humidity_sensor(
    override_id: Option<&str>,
    entities: Option<&EntitySet>,
) -> hauto::Result<Sensor<SensorValue<f64>>> {
    match override_id {
        Some(entity_id) => Sensor::new(entity_id),
        None => entities
            .expect("entities loaded when humidity override is absent")
            .query()
            .domain("sensor")
            .device_class("humidity")
            .exactly_one()?
            .sensor(),
    }
}

fn status_entity(
    bathroom: &BathroomSpec,
    area: Option<&hauto::AreaInfo>,
) -> hauto::Result<EntityId> {
    match bathroom.humidity_status.as_deref() {
        Some(entity_id) => EntityId::new(entity_id),
        None => derived_status_entity(
            area.expect("area resolved when status override is absent")
                .id()
                .as_str(),
        ),
    }
}

fn derived_status_entity(area_id: &str) -> hauto::Result<EntityId> {
    EntityId::new(format!("sensor.hauto_{area_id}_excess_humidity"))
}

fn missing(name: &str) -> String {
    format!("missing required environment variable `{name}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explicit_bathroom() -> BathroomSpec {
        BathroomSpec::new(
            "Bathroom",
            None,
            Some("sensor.temperature".into()),
            Some("sensor.humidity".into()),
            Some("binary_sensor.occupancy".into()),
            Some("sensor.humidity_status".into()),
            "HAUTO_BATHROOM_1_AREA",
        )
        .unwrap()
    }

    #[test]
    fn fully_explicit_bathroom_does_not_require_area() {
        let bathroom = explicit_bathroom();
        assert!(!bathroom.needs_area_for_humidity());
        assert!(!bathroom.needs_area_for_fan());
        assert_eq!(bathroom.display_name(), "Bathroom");
    }

    #[test]
    fn area_name_is_used_as_the_bathroom_display_name() {
        let bathroom = BathroomSpec::new(
            "HAUTO_BATHROOM_1",
            Some("Main Bathroom".into()),
            None,
            None,
            None,
            None,
            "HAUTO_BATHROOM_1_AREA",
        )
        .unwrap();

        assert_eq!(bathroom.display_name(), "Main Bathroom");
    }

    #[test]
    fn missing_override_requires_the_corresponding_area() {
        let error = BathroomSpec::new(
            "Bathroom",
            None,
            Some("sensor.temperature".into()),
            Some("sensor.humidity".into()),
            Some("binary_sensor.occupancy".into()),
            None,
            "HAUTO_BATHROOM_1_AREA",
        )
        .unwrap_err();
        assert_eq!(
            error,
            "missing required environment variable `HAUTO_BATHROOM_1_AREA`"
        );
    }

    #[test]
    fn explicit_ambient_and_fan_do_not_require_discovery_settings() {
        assert!(
            AmbientSpec::new(
                None,
                Some("sensor.temperature".into()),
                Some("sensor.humidity".into())
            )
            .is_ok()
        );
        assert!(FanSpec::new(None, Some("switch.fan".into())).is_ok());
    }

    #[test]
    fn unresolved_ambient_and_fan_roles_require_discovery_settings() {
        assert_eq!(
            AmbientSpec::new(None, Some("sensor.temperature".into()), None).unwrap_err(),
            "missing required environment variable `HAUTO_AMBIENT_AREA`"
        );
        assert_eq!(
            FanSpec::new(None, None).unwrap_err(),
            "missing required environment variable `HAUTO_EXHAUST_FAN_NAME`"
        );
    }

    #[test]
    fn status_id_is_stable_for_an_explicit_override() {
        let bathroom = explicit_bathroom();
        assert_eq!(
            status_entity(&bathroom, None).unwrap().as_str(),
            "sensor.humidity_status"
        );
    }

    #[test]
    fn derived_status_id_is_stable_for_an_area_id() {
        assert_eq!(
            derived_status_entity("main_bathroom").unwrap().as_str(),
            "sensor.hauto_main_bathroom_excess_humidity"
        );
    }
}
