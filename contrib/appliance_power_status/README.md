# Appliance power status

This example derives a human-friendly appliance status from a numeric Home
Assistant power sensor.

It is useful for appliances such as washing machines and dryers where power
draw is a better signal than a built-in status entity. The automation watches a
source power sensor and publishes a derived status sensor with one of four
states:

- `running` when power is at or above `idle_below`;
- `idle` when power stays below `idle_below` for `idle_delay`;
- `off` when power stays below `off_below` for `off_delay`;
- `unknown` when the power sensor is missing, empty, `unknown`, or
  `unavailable`.

Malformed numeric power states are treated as errors. This keeps integration or
template mistakes visible instead of silently mapping them to `unknown`.

## Status model

The automation is threshold based:

```text
power >= idle_below              => running
off_below <= power < idle_below  => candidate idle
power < off_below                => candidate off
missing/unknown/unavailable      => unknown
```

`idle` and `off` are delayed states. The power reading must remain below the
relevant threshold for the configured delay before the status is published. If
power rises above the threshold during the delay, the pending status is
interrupted and the automation reclassifies from the current power state.

That shape handles normal appliance cycles such as:

```text
off -> running -> idle -> running -> idle -> off
```

## Configuration

Each appliance is configured with an `AppliancePowerStatusConfig` value:

- `power_entity`: source `Sensor<SensorValue<f64>>` power sensor;
- `status_entity`: derived status entity to publish;
- `friendly_name`: friendly name for the derived status entity;
- `off_below`: power threshold below which the appliance is considered off;
- `idle_below`: power threshold below which the appliance may be idle;
- `off_delay`: how long power must stay below `off_below` before publishing
  `off`;
- `idle_delay`: how long power must stay below `idle_below` before publishing
  `idle`;
- `icons`: Material Design Icons to publish with each derived status.

The runnable example in [`main.rs`](main.rs) defines two appliances:

- `sensor.laundry_washing_machine_power` -> `sensor.washing_machine_status`
- `sensor.laundry_dryer_power` -> `sensor.dryer_status`

Adjust those entity ids and thresholds for your own Home Assistant setup.

## Running the example

The example reads Home Assistant connection details from environment variables:

```sh
export HOME_ASSISTANT_URL='http://homeassistant.local:8123'
export HOME_ASSISTANT_TOKEN='...'
cargo run --example appliance_power_status
```

The reusable automation logic lives in
[`appliance_power_status.rs`](appliance_power_status.rs). Copy that file into
another hauto project if you want to construct appliance configs from your own
application bootstrap code.

## Implementation notes

The automation uses hauto primitives directly:

- `Sensor::<SensorValue<f64>>` decodes numeric power readings while representing
  `unknown`, `unavailable`, and empty states as typed availability values.
- `get(&ctx)` reads and decodes the current power state.
- `expect_matching(...).for_at_least(...)` implements delayed `idle` and `off`
  holds.
- `next_change(&ctx)` waits for the next source power change before
  reclassifying after a published status.
- `set_state_raw` publishes the derived status entity through Home Assistant's
  REST states API.

The derived status entity is a Home Assistant state-machine entry, not a
persistent entity registry entry. If you want a persistent entity registry
entity, expose the status through a Home Assistant integration or helper
instead of publishing it only through the states API.
