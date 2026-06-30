//! Control a shared bathroom exhaust fan from humidity and occupancy.
//!
//! See `examples/bathroom_exhaust_fan/README.md` for the entity mapping and
//! behavior details.

use std::{env, error::Error};

use bathroom_exhaust_fan::{
    AmbientSensors, BathroomConfig, BathroomExhaustFan, BathroomExhaustFanConfig, QuietHours,
    Settings,
};
use hauto::{App, BinarySensor, EntityId, Sensor, SensorValue};

mod bathroom_exhaust_fan;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;

    let settings = Settings {
        quiet_hours: QuietHours {
            start_minute: optional_env_u32("HAUTO_QUIET_START_MINUTE")?.unwrap_or(0),
            end_minute: optional_env_u32("HAUTO_QUIET_END_MINUTE")?.unwrap_or(8 * 60),
            utc_offset_minutes: optional_env_i32("HAUTO_LOCAL_UTC_OFFSET_MINUTES")?.unwrap_or(0),
        },
        ..Settings::default()
    };

    let config = BathroomExhaustFanConfig {
        fan_entity: EntityId::new(required_env("HAUTO_EXHAUST_FAN")?)?,
        ambient: AmbientSensors {
            temperature: Sensor::<SensorValue<f64>>::new(required_env("HAUTO_AMBIENT_TEMP")?)?,
            humidity: Sensor::<SensorValue<f64>>::new(required_env("HAUTO_AMBIENT_HUMIDITY")?)?,
        },
        bathrooms: [
            BathroomConfig {
                name: "bathroom 1".to_string(),
                temperature: Sensor::<SensorValue<f64>>::new(required_env(
                    "HAUTO_BATHROOM_1_TEMP",
                )?)?,
                humidity: Sensor::<SensorValue<f64>>::new(required_env(
                    "HAUTO_BATHROOM_1_HUMIDITY",
                )?)?,
                occupancy: BinarySensor::new(required_env("HAUTO_BATHROOM_1_OCCUPANCY")?)?,
            },
            BathroomConfig {
                name: "bathroom 2".to_string(),
                temperature: Sensor::<SensorValue<f64>>::new(required_env(
                    "HAUTO_BATHROOM_2_TEMP",
                )?)?,
                humidity: Sensor::<SensorValue<f64>>::new(required_env(
                    "HAUTO_BATHROOM_2_HUMIDITY",
                )?)?,
                occupancy: BinarySensor::new(required_env("HAUTO_BATHROOM_2_OCCUPANCY")?)?,
            },
        ],
        settings,
    };

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("bathroom exhaust fan", move |ctx| {
            let automation = BathroomExhaustFan::new(config.clone());
            async move { automation.run(ctx).await }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}

fn optional_env_u32(name: &'static str) -> Result<Option<u32>, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|error| format!("invalid `{name}`: {error}").into()),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("invalid `{name}`: {error}").into()),
    }
}

fn optional_env_i32(name: &'static str) -> Result<Option<i32>, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) => value
            .parse::<i32>()
            .map(Some)
            .map_err(|error| format!("invalid `{name}`: {error}").into()),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("invalid `{name}`: {error}").into()),
    }
}
