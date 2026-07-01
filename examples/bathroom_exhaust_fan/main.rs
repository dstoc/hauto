//! Control a shared bathroom exhaust fan from derived humidity status sensors
//! and occupancy.
//!
//! See `examples/bathroom_exhaust_fan/README.md` for the entity mapping and
//! behavior details.

use std::{env, error::Error};

use fan_control::{FanBathroomConfig, FanControl, FanControlConfig, FanSettings, QuietHours};
use hauto::{App, BinarySensor, EntityId, Sensor, SensorValue, Switch};
use humidity_status::{HumiditySettings, HumidityStatus, HumidityStatusConfig};

mod fan_control;
mod humidity_status;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;

    let quiet_hours = QuietHours {
        start_minute: optional_env_u32("HAUTO_QUIET_START_MINUTE")?.unwrap_or(0),
        end_minute: optional_env_u32("HAUTO_QUIET_END_MINUTE")?.unwrap_or(8 * 60),
        utc_offset_minutes: optional_env_i32("HAUTO_LOCAL_UTC_OFFSET_MINUTES")?.unwrap_or(0),
    };

    let bathroom_1_humidity_status =
        EntityId::new(required_env("HAUTO_BATHROOM_1_HUMIDITY_STATUS")?)?;
    let bathroom_2_humidity_status =
        EntityId::new(required_env("HAUTO_BATHROOM_2_HUMIDITY_STATUS")?)?;
    let ambient_temperature = Sensor::<SensorValue<f64>>::new(required_env("HAUTO_AMBIENT_TEMP")?)?;
    let ambient_humidity =
        Sensor::<SensorValue<f64>>::new(required_env("HAUTO_AMBIENT_HUMIDITY")?)?;

    let bathroom_1_temperature =
        Sensor::<SensorValue<f64>>::new(required_env("HAUTO_BATHROOM_1_TEMP")?)?;
    let bathroom_1_humidity =
        Sensor::<SensorValue<f64>>::new(required_env("HAUTO_BATHROOM_1_HUMIDITY")?)?;
    let bathroom_1_occupancy = BinarySensor::new(required_env("HAUTO_BATHROOM_1_OCCUPANCY")?)?;

    let bathroom_2_temperature =
        Sensor::<SensorValue<f64>>::new(required_env("HAUTO_BATHROOM_2_TEMP")?)?;
    let bathroom_2_humidity =
        Sensor::<SensorValue<f64>>::new(required_env("HAUTO_BATHROOM_2_HUMIDITY")?)?;
    let bathroom_2_occupancy = BinarySensor::new(required_env("HAUTO_BATHROOM_2_OCCUPANCY")?)?;

    let bathroom_1_humidity = HumidityStatusConfig {
        name: "Bathroom 1".to_string(),
        status_entity: bathroom_1_humidity_status.clone(),
        bathroom_temperature: bathroom_1_temperature,
        bathroom_humidity: bathroom_1_humidity,
        ambient_temperature: ambient_temperature.clone(),
        ambient_humidity: ambient_humidity.clone(),
        settings: HumiditySettings::default(),
    };
    let bathroom_2_humidity = HumidityStatusConfig {
        name: "Bathroom 2".to_string(),
        status_entity: bathroom_2_humidity_status.clone(),
        bathroom_temperature: bathroom_2_temperature,
        bathroom_humidity: bathroom_2_humidity,
        ambient_temperature,
        ambient_humidity,
        settings: HumiditySettings::default(),
    };
    let fan_control = FanControlConfig {
        fan: Switch::new(required_env("HAUTO_EXHAUST_FAN")?)?,
        bathrooms: [
            FanBathroomConfig {
                name: "Bathroom 1".to_string(),
                humidity_status: Sensor::<String>::new(bathroom_1_humidity_status.to_string())?,
                occupancy: bathroom_1_occupancy,
            },
            FanBathroomConfig {
                name: "Bathroom 2".to_string(),
                humidity_status: Sensor::<String>::new(bathroom_2_humidity_status.to_string())?,
                occupancy: bathroom_2_occupancy,
            },
        ],
        settings: FanSettings {
            quiet_hours,
            ..FanSettings::default()
        },
    };

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("bathroom 1 humidity status", move |ctx| {
            let automation = HumidityStatus::new(bathroom_1_humidity.clone());
            async move { automation.run(ctx).await }
        })
        .automation_fn("bathroom 2 humidity status", move |ctx| {
            let automation = HumidityStatus::new(bathroom_2_humidity.clone());
            async move { automation.run(ctx).await }
        })
        .automation_fn("bathroom exhaust fan control", move |ctx| {
            let automation = FanControl::new(fan_control.clone());
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
