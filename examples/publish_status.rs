//! Publish an AppDaemon-style status entity to Home Assistant.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_STATUS_ENTITY`: status entity id, for example `sensor.hauto_status`
//!
//! This uses Home Assistant's REST states API. State publishing through that API
//! is ephemeral: it creates or updates the runtime state machine entry, but it
//! does not create an entity registry entry or make the entity survive a Home
//! Assistant restart.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example publish_status
//! ```

use std::{env, error::Error, time::Duration};

use hauto::{App, EntityId, StateWrite};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let status_entity = EntityId::new(required_env("HAUTO_STATUS_ENTITY")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("publish hauto status", move |ctx| {
            let status_entity = status_entity.clone();

            async move {
                let mut update_count = 0_u64;

                loop {
                    update_count += 1;

                    let state = StateWrite::new(
                        "running",
                        json!({
                            "friendly_name": "hauto status",
                            "icon": "mdi:robot",
                            "update_count": update_count,
                        }),
                    )?;

                    let result = ctx
                        .home_assistant()
                        .set_state_raw(&status_entity, state)
                        .await?;

                    println!("{status_entity} is running ({result:?})");
                    ctx.sleep(Duration::from_secs(30)).await?;
                }
            }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
