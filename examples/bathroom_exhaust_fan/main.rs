//! Control a shared bathroom exhaust fan from derived humidity status sensors
//! and occupancy.
//!
//! See `examples/bathroom_exhaust_fan/README.md` for the entity mapping and
//! behavior details.

use std::{env, error::Error};

use discovery::{AmbientSpec, BathroomSpec, FanSpec, resolve_fan_config, resolve_humidity_config};
use fan_control::{FanControl, FanSettings, QuietHours};
use hauto::App;
use humidity_status::{HumiditySettings, HumidityStatus};

mod discovery;
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

    let bathrooms = [
        bathroom_spec("Bathroom 1", "HAUTO_BATHROOM_1")?,
        bathroom_spec("Bathroom 2", "HAUTO_BATHROOM_2")?,
    ];
    let ambient = AmbientSpec::new(
        optional_env("HAUTO_AMBIENT_AREA")?,
        optional_env("HAUTO_AMBIENT_TEMP")?,
        optional_env("HAUTO_AMBIENT_HUMIDITY")?,
    )?;
    let fan = FanSpec::new(
        optional_env("HAUTO_EXHAUST_FAN_NAME")?,
        optional_env("HAUTO_EXHAUST_FAN")?,
    )?;

    let bathroom_1 = bathrooms[0].clone();
    let bathroom_2 = bathrooms[1].clone();
    let bathroom_1_ambient = ambient.clone();
    let bathroom_2_ambient = ambient;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("bathroom 1 humidity status", move |ctx| {
            let bathroom = bathroom_1.clone();
            let ambient = bathroom_1_ambient.clone();
            async move {
                let config =
                    resolve_humidity_config(&ctx, &bathroom, &ambient, HumiditySettings::default())
                        .await?;
                HumidityStatus::new(config).run(ctx).await
            }
        })
        .automation_fn("bathroom 2 humidity status", move |ctx| {
            let bathroom = bathroom_2.clone();
            let ambient = bathroom_2_ambient.clone();
            async move {
                let config =
                    resolve_humidity_config(&ctx, &bathroom, &ambient, HumiditySettings::default())
                        .await?;
                HumidityStatus::new(config).run(ctx).await
            }
        })
        .automation_fn("bathroom exhaust fan control", move |ctx| {
            let bathrooms = bathrooms.clone();
            let fan = fan.clone();
            async move {
                let config = resolve_fan_config(
                    &ctx,
                    &bathrooms,
                    &fan,
                    FanSettings {
                        quiet_hours,
                        ..FanSettings::default()
                    },
                )
                .await?;
                FanControl::new(config).run(ctx).await
            }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}

fn optional_env(name: &'static str) -> Result<Option<String>, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("invalid `{name}`: {error}").into()),
    }
}

fn bathroom_spec(name: &str, prefix: &str) -> Result<BathroomSpec, Box<dyn Error>> {
    let area_variable = format!("{prefix}_AREA");
    BathroomSpec::new(
        name,
        optional_env_name(&area_variable)?,
        optional_env_name(&format!("{prefix}_TEMP"))?,
        optional_env_name(&format!("{prefix}_HUMIDITY"))?,
        optional_env_name(&format!("{prefix}_OCCUPANCY"))?,
        optional_env_name(&format!("{prefix}_HUMIDITY_STATUS"))?,
        &area_variable,
    )
    .map_err(Into::into)
}

fn optional_env_name(name: &str) -> Result<Option<String>, Box<dyn Error>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("invalid `{name}`: {error}").into()),
    }
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
