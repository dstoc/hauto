//! Toggle a Home Assistant light every 10 seconds.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_LIGHT`: light entity id, for example `light.living_room`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example light_toggle
//! ```

use std::{env, error::Error, time::Duration};

use hauto::{App, Error as HautoError, Light, LightTurnOff, LightTurnOn};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;
    let toggle_light = light.clone();
    let print_light = light.clone();

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("toggle light every 10 seconds", move |ctx| {
            let light = toggle_light.clone();

            async move {
                loop {
                    light.turn_on(&ctx, LightTurnOn::default()).await?;
                    ctx.sleep(Duration::from_secs(10)).await?;

                    light.turn_off(&ctx, LightTurnOff::default()).await?;
                    ctx.sleep(Duration::from_secs(10)).await?;
                }
            }
        })
        .automation_fn("print light on/off changes", move |ctx| {
            let light = print_light.clone();

            async move {
                let mut changes = ctx.light_changes(&light);

                while let Some(event) = changes.next().await {
                    let event = event.map_err(HautoError::EventStream)?;
                    let Some(new_state) = event.new_state else {
                        println!("{} was removed", event.entity_id);
                        continue;
                    };

                    match new_state.state.as_str() {
                        "on" => println!("{} turned on", event.entity_id),
                        "off" => println!("{} turned off", event.entity_id),
                        other => println!("{} changed to {other}", event.entity_id),
                    }
                }

                Ok(())
            }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
