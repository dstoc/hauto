//! Turn a light on or off from a numeric temperature sensor threshold.
//!
//! This demonstrates the current sensor API with `Sensor::<f64>::new`. At the
//! moment, numeric sensor decoding is intentionally simple: the example reads
//! the Home Assistant state string and parses it as `f64`.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_TEMPERATURE_SENSOR`: sensor entity id, for example `sensor.office_temperature`
//! - `HAUTO_THRESHOLD`: numeric threshold, for example `24.5`
//! - `HAUTO_LIGHT`: light entity id, for example `light.office`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example temperature_threshold
//! ```

use std::{env, error::Error};

use hauto::{
    App, Context, EntityState, Error as HautoError, Light, LightTurnOff, LightTurnOn, Sensor,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let sensor = Sensor::<f64>::new(required_env("HAUTO_TEMPERATURE_SENSOR")?)?;
    let threshold = required_env("HAUTO_THRESHOLD")?.parse::<f64>()?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("temperature threshold light control", move |ctx| {
            let sensor = sensor.clone();
            let light = light.clone();

            async move {
                match sensor.state(&ctx).await {
                    Ok(state) => apply_threshold(&ctx, &light, threshold, &state).await?,
                    Err(HautoError::EntityNotFound(entity_id)) => {
                        println!("{entity_id}: no initial state found");
                    }
                    Err(error) => return Err(error),
                }

                let mut changes = ctx.state_changes(sensor.entity_id());

                while let Some(event) = changes.next().await {
                    let event = event.map_err(HautoError::EventStream)?;
                    let Some(new_state) = event.new_state else {
                        println!("{} was removed; waiting for it to return", event.entity_id);
                        continue;
                    };

                    apply_threshold(&ctx, &light, threshold, &new_state).await?;
                }

                Ok(())
            }
        })
        .run()
        .await?;

    Ok(())
}

async fn apply_threshold(
    ctx: &Context,
    light: &Light,
    threshold: f64,
    state: &EntityState,
) -> hauto::Result<()> {
    let temperature = match state.state.parse::<f64>() {
        Ok(temperature) => temperature,
        Err(_) => {
            println!(
                "{}: ignoring non-numeric temperature state {:?}",
                state.entity_id, state.state
            );
            return Ok(());
        }
    };

    if temperature >= threshold {
        println!(
            "{}: {temperature} >= {threshold}; turning {} on",
            state.entity_id,
            light.entity_id()
        );
        light.turn_on(ctx, LightTurnOn::default()).await?;
    } else {
        println!(
            "{}: {temperature} < {threshold}; turning {} off",
            state.entity_id,
            light.entity_id()
        );
        light.turn_off(ctx, LightTurnOff::default()).await?;
    }

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
