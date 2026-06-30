# Appliance power status conversion sketch

This sketches a hauto conversion of an AppDaemon appliance-power monitor. The
original automation watches a power sensor and publishes a derived status
sensor:

- `running` when power is at or above `idle_below`;
- `idle` when power is between `off_below` and `idle_below`, either
  immediately from off/unknown or after `idle_delay` while dropping from
  running;
- `off` when power is below `off_below`, either immediately at startup/unknown
  or after `off_delay` while dropping from active power;
- `unknown` when the power sensor is missing, empty, `unknown`, `unavailable`,
  or non-numeric.

The Rust sketch intentionally does not keep the original callback/timer shape.
Instead, it uses a reclassifying loop:

1. read the current power;
2. publish immediate states (`unknown` and `running`);
3. require candidate delayed states (`idle` and `off`) to hold for their delay;
4. after each change, interruption, or published status, loop back and classify
   the current power again.

That shape handles appliance cycles such as `off -> running -> idle -> running`
without treating the status flow as one-way.

## AppDaemon config shape

```yaml
washing_machine_status:
  module: appliance_power_status
  class: AppliancePowerStatus

  power_entity: sensor.laundry_washing_machine_power
  status_entity: sensor.washing_machine_status
  friendly_name: Washing Machine Status

  off_below: 3
  idle_below: 10
  off_delay: 300
  idle_delay: 30

  icons:
    off: mdi:washing-machine-off
    idle: mdi:pause-circle
    running: mdi:washing-machine
    unknown: mdi:help-circle

dryer_status:
  module: appliance_power_status
  class: AppliancePowerStatus

  power_entity: sensor.laundry_dryer_power
  status_entity: sensor.dryer_status
  friendly_name: Dryer Status

  off_below: 1
  idle_below: 10
  off_delay: 300
  idle_delay: 30

  icons:
    off: mdi:tumble-dryer-off
    idle: mdi:pause-circle
    running: mdi:tumble-dryer
    unknown: mdi:help-circle
```

## hauto shape

The reusable automation logic is in
[`appliance_power_status.rs`](appliance_power_status.rs). That file is intended
to be easy to copy into another hauto project.

The runnable bootstrap is in [`main.rs`](main.rs). It constructs the washing
machine and dryer configs, registers them with `App`, and reads the Home
Assistant URL/token from the environment. It can be compile-checked and run as
a Cargo example:

```sh
cargo run --example appliance_power_status
```

The conversion uses:

- `Sensor::<f64>` for the source power entity;
- `EntityId` + `set_state_raw` for the derived status sensor;
- `Context::state_changes` for the event stream;
- cancellation-aware `tokio::select!` with `ctx.cancelled()` for held
  thresholds.

The important simplification is that there are no explicit idle/off timer
handles. A pending delayed state is just a held predicate over the power stream.
If the predicate is interrupted, the automation loops and reclassifies from the
current Home Assistant state.

## Open migration questions

- hauto has no config loader yet. This sketch constructs the two appliance
  definitions in Rust instead of reading `apps.yaml`.
- `set_state_raw` mirrors `set_state(..., check_existence=False)` closely: it
  publishes state through Home Assistant's REST states API and does not create a
  persistent entity registry entry.
- The current hauto `Sensor::<f64>` decoder treats non-numeric states as
  errors. The AppDaemon automation treats them as `unknown`. This sketch uses
  raw state strings for the power sensor so it can preserve that behavior.
- There is no entity-level `next_change` helper yet, so the sketch uses
  `ctx.state_changes(power.entity_id())` directly.
- There is no unknown-tolerant numeric sensor type yet, such as
  `Sensor<Option<f64>>`. Adding one would let this example use typed waits and
  expectations more directly.
