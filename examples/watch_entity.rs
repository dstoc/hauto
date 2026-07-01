//! Watch state-change events for one Home Assistant entity.
//!
//! This is a minimal event-stream example. It subscribes to the framework's
//! state-change stream for a single entity and prints each old-state to
//! new-state transition, including entity deletion.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_ENTITY`: entity id to watch, for example `sensor.office_temperature`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example watch_entity
//! ```

use std::{env, error::Error};

use hauto::{App, EntityId, Error as HautoError, state::EntityState};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let entity_id = EntityId::new(required_env("HAUTO_ENTITY")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("watch entity state changes", move |ctx| {
            let entity_id = entity_id.clone();

            async move {
                let mut changes = ctx.state_changes(&entity_id);

                while let Some(event) = changes.next().await {
                    let event = event.map_err(HautoError::EventStream)?;
                    let old_state = state_label(event.old_state.as_ref());
                    let new_state = state_label(event.new_state.as_ref());

                    if event.new_state.is_some() {
                        println!("{}: {old_state} -> {new_state}", event.entity_id);
                    } else {
                        println!("{}: {old_state} -> <deleted>", event.entity_id);
                    }
                }

                Ok(())
            }
        })
        .run()
        .await?;

    Ok(())
}

fn state_label(state: Option<&EntityState>) -> &str {
    state.map_or("<missing>", |state| state.state.as_str())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
