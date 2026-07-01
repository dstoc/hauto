# hauto

`hauto` is an async Rust automation framework for Home Assistant.

It provides typed entity handles, cancellation-aware timers, state-change
streams, typed wait primitives, service calls, and REST state publishing. The
default entrypoint is `App`, which connects to Home Assistant, keeps an
in-memory state cache, runs registered automations, and restarts automations
after a Home Assistant connection generation is replaced.

The API is currently early and intentionally focused on defining the framework
shape.

See the [API documentation](https://docs.rs/hauto/latest/hauto/) for the
runtime model, entity and state semantics, waits and expectations, discovery,
service calls, cancellation, and reconnect behavior.

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

## Examples

Examples use these connection environment variables:

```sh
export HOME_ASSISTANT_URL='http://homeassistant.local:8123'
export HOME_ASSISTANT_TOKEN='...'
```

Available examples:

- [`light_toggle`](examples/light_toggle.rs) — toggle a light every 10 seconds
  and print state changes.
- [`occupancy_light`](examples/occupancy_light.rs) — occupancy sensor with
  delayed turn-off.
- [`appliance_power_status`](examples/appliance_power_status/README.md) —
  derive washer/dryer status from power sensors.
- [`bathroom_exhaust_fan`](examples/bathroom_exhaust_fan/README.md) — shared
  fan control from humidity and occupancy.
- [`temperature_threshold`](examples/temperature_threshold.rs) — typed numeric
  sensor predicate.
- [`global_wait`](examples/global_wait.rs) — compound predicate over
  temperature and humidity sensors.
- [`timer_cancel`](examples/timer_cancel.rs) — delayed action cancellation.
- [`watch_entity`](examples/watch_entity.rs) — print state-change events for
  one entity.
- [`publish_status`](examples/publish_status.rs) — publish an ephemeral status
  entity through REST state APIs.
- [`raw_service`](examples/raw_service.rs) — call an arbitrary Home Assistant
  service.
- [`raw_command`](examples/raw_command.rs) — send an arbitrary WebSocket
  command.

Run an example with:

```sh
cargo run --example light_toggle
```

## Scope

The current API focuses on the automation framework and Home Assistant
integration. Proc macros, a CLI, a template DSL, non-Home-Assistant backends,
and domain-specific wrappers for every integration are not currently in scope.
