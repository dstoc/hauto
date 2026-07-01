# hauto

`hauto` is an async Rust automation framework for Home Assistant.

It provides typed entity handles, cancellation-aware timers, state-change
streams, typed wait primitives, service calls, and REST state publishing. The
default entrypoint is `App`, which connects to Home Assistant, keeps an
in-memory state cache, runs registered automations, and restarts automations
after a Home Assistant connection generation is replaced.

The API is currently early and intentionally focused on defining the framework
shape.

## Basic automation

```rust
use std::time::Duration;

use hauto::{App, BinarySensor, HoldResult, Light, LightTurnOff, LightTurnOn};

#[tokio::main(flavor = "current_thread")]
async fn main() -> hauto::Result<()> {
    let occupancy = BinarySensor::new("binary_sensor.office_occupancy")?;
    let light = Light::new("light.office")?;

    App::new("http://homeassistant.local:8123", "long-lived-access-token")
        .automation_fn("office occupancy light", move |ctx| {
            let occupancy = occupancy.clone();
            let light = light.clone();

            async move {
                loop {
                    occupancy.wait_until_on(&ctx).await?;
                    light.turn_on(&ctx, LightTurnOn::default()).await?;

                    if matches!(
                        occupancy
                            .expect_off(&ctx)
                            .for_at_least(Duration::from_secs(30))
                            .await?,
                        HoldResult::Held
                    ) {
                        light.turn_off(&ctx, LightTurnOff::default()).await?;
                    }
                }
            }
        })
        .run()
        .await
}
```

## Runtime model

`App` is the high-level entrypoint:

1. Connects to Home Assistant over WebSocket.
2. Fetches the initial state snapshot.
3. Starts a Home Assistant `state_changed` subscription.
4. Runs each registered automation with a cloneable `Context`.
5. Cancels the current generation and restarts automations when the connection
   is replaced or lost.

Automations normally run loops. If an automation task returns an error other
than cancellation, `App::run` returns an `Error::AutomationTask`.

`Context` is the automation handle. It exposes:

- cancellation-aware `sleep`, `timeout`, `run_after`, and `spawn`
- entity state-change streams
- global state predicate waits
- access to `hauto::client::HomeAssistantClient` for raw service, command, and
  state APIs

## Entity handles

Typed handles validate entity-id domains when constructed, but they do not check
that the entity currently exists in Home Assistant:

```rust
let light = hauto::Light::new("light.office")?;
let temperature = hauto::Sensor::<f64>::new("sensor.office_temperature")?;
```

Existence and state validity are checked when reading state, waiting for state,
or calling Home Assistant.

Use `get(&ctx)` to fetch and decode the current state for a typed entity:

```rust
let temperature = temperature.get(&ctx).await?;
```

Use `next_change(&ctx)` to wait for the next state change for that entity and
decode the new state:

```rust
let temperature = temperature.next_change(&ctx).await?;
```

Use `read(&hauto::state::StateCache)` inside global state predicates, where the
current cache view is already available synchronously.

Initial typed handles include:

- `BinarySensor`
- `Light`
- `Switch`
- `Sensor<f64>`
- `Sensor<String>`

## Waiting for state

Binary-style entities support direct state waits:

```rust
light.wait_until_on(&ctx).await?;
light.wait_until_off(&ctx).within(Duration::from_secs(10)).await?;
```

Numeric and string sensors support typed predicates:

```rust
temperature
    .wait_until_matching(&ctx, |value| *value > 30.0)
    .for_at_least(Duration::from_secs(60))
    .await?;
```

Wait builders support:

- `.for_at_least(duration)` — the condition must remain true for the duration.
- `.within(duration)` — returns `Ok(WaitResult::TimedOut)` if the timeout
  expires.
- `.require_transition()` on entity waits — ignore an already-satisfied initial
  state until the condition first becomes false and then true again.

Expectations are immediate checks with optional hold semantics:

```rust
match light.expect_on(&ctx).for_at_least(Duration::from_secs(5)).await? {
    hauto::HoldResult::Held => {}
    hauto::HoldResult::NotSatisfied { actual } => {
        println!("light was initially {actual:?}");
    }
    hauto::HoldResult::Interrupted { actual } => {
        println!("light changed to {actual:?} before the hold completed");
    }
}
```

## Global state predicates

Use `Context::wait_until_state` when a condition spans multiple entities and
must be true at the same time:

```rust
ctx.wait_until_state(move |state| {
    let Some(t) = temperature.read(state)? else {
        return Ok(false);
    };
    let Some(h) = humidity.read(state)? else {
        return Ok(false);
    };

    Ok(t >= 24.0 && h <= 55.0)
})
.for_at_least(Duration::from_secs(30))
.await?;
```

The predicate runs once against the current cache and then after every Home
Assistant state change. Keep predicates synchronous and cheap. Prefer
entity-specific waits for single-entity conditions.

## Calling services

Typed light service helpers are available:

```rust
light
    .turn_on(
        &ctx,
        hauto::LightTurnOn {
            brightness_pct: Some(75),
            ..Default::default()
        },
    )
    .await?;
```

Escape hatches are available through `hauto::client::HomeAssistantClient`:

```rust
ctx.home_assistant()
    .call_service_raw("notify", "persistent_notification", serde_json::json!({
        "message": "hello from hauto"
    }))
    .await?;
```

## Publishing state

`set_state_raw` and `delete_state_raw` use Home Assistant's REST states API.
These APIs create, update, or delete runtime state-machine entries; they do not
create entity registry entries and are not persistent across Home Assistant
restart.

```rust
let entity = hauto::EntityId::new("sensor.hauto_status")?;
ctx.home_assistant()
    .set_state_raw(
        &entity,
        hauto::state::StateWrite::new(
            "ready",
            serde_json::json!({
                "friendly_name": "hauto status",
                "icon": "mdi:check-circle",
            }),
        )?,
    )
    .await?;
```

## Examples

Examples read connection details from environment variables:

```sh
export HOME_ASSISTANT_URL='http://homeassistant.local:8123'
export HOME_ASSISTANT_TOKEN='...'
```

Available examples:

- `light_toggle` — turn a light on and off every 10 seconds and print state changes.
- `occupancy_light` — occupancy sensor with delayed turn-off.
- `appliance_power_status` — derive washer/dryer status from power sensors.
- `bathroom_exhaust_fan` — shared fan control from humidity and occupancy.
- `temperature_threshold` — typed numeric sensor predicate.
- `global_wait` — compound predicate over temperature and humidity sensors.
- `timer_cancel` — delayed action cancellation.
- `watch_entity` — print state-change events for one entity.
- `publish_status` — publish an ephemeral status entity through REST state APIs.
- `raw_service` — call an arbitrary Home Assistant service.
- `raw_command` — send an arbitrary WebSocket command.

Run an example with:

```sh
cargo run --example light_toggle
```

## Current non-goals

- Proc macros and a CLI crate.
- A Home Assistant template DSL.
- Non-Home-Assistant backends.
- Domain-specific wrappers for every Home Assistant integration.
