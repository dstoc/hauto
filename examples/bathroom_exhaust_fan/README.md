# Bathroom exhaust fan

This example controls one shared exhaust fan for two bathrooms by composing
small automations through Home Assistant state.

It has three automations:

1. bathroom 1 humidity status publisher
2. bathroom 2 humidity status publisher
3. shared fan controller

The humidity automations publish derived status sensors:

```text
sensor.bathroom_1_excess_humidity = normal | humid | unknown
sensor.bathroom_2_excess_humidity = normal | humid | unknown
```

The fan controller then consumes those status sensors plus occupancy sensors:

```text
fan_on =
    bathroom_1_humidity_is_humid
    OR bathroom_2_humidity_is_humid
    OR daytime_bathroom_1_occupancy_demand
    OR daytime_bathroom_2_occupancy_demand
```

During quiet hours, occupancy is ignored:

```text
00:00 <= local time < 08:00:
    fan_on =
        bathroom_1_humidity_is_humid
        OR bathroom_2_humidity_is_humid
```

That split keeps the humidity model observable and reusable. You can see the
derived humidity status and diagnostic attributes in Home Assistant, while the
fan controller remains a small arbitration automation.

## Humidity status publisher

Each bathroom compares its humidity against a nearby ambient room, using
absolute humidity where temperature is available:

```text
bathroom_absolute_humidity = f(bathroom_temp, bathroom_rh)
ambient_absolute_humidity  = f(ambient_temp, ambient_rh)
humidity_excess = bathroom_absolute_humidity - ambient_absolute_humidity
```

Humidity status becomes `humid` when any of these are true:

```text
absolute humidity excess > 2.0 g/m³
OR bathroom RH > ambient RH + 12%
OR bathroom RH > 70%
OR bathroom RH rises by > 6% over 5 minutes
OR bathroom absolute humidity rises by > 0.6 g/m³ over 5 minutes
```

Humidity status returns to `normal` only after the bathroom has dried back down
for 5 continuous minutes and the minimum humid runtime has elapsed:

```text
absolute humidity excess < 0.8 g/m³
AND bathroom RH < ambient RH + 6%
AND bathroom RH < 65%
```

The start and clear thresholds are deliberately different. That hysteresis
keeps the status sensor from flapping around a single threshold.

The status sensor attributes include:

- `reason`
- `bathroom_relative_humidity`
- `ambient_relative_humidity`
- `bathroom_absolute_humidity`
- `ambient_absolute_humidity`
- `absolute_humidity_excess`
- `relative_humidity_excess`

Rate-of-rise history is sampled at most once every 30 seconds. Sensor-change
events still trigger immediate threshold and rate re-evaluation using the
current reading, but closely spaced events do not add redundant history
samples. Poll ticks ensure samples are also recorded while sensor values remain
unchanged.

## Fan controller

Humidity is the primary safety trigger. Occupancy is a comfort/odour trigger
only outside quiet hours, so a normal overnight bathroom visit does not start
the fan, but an overnight shower still does.

Outside quiet hours:

```text
occupied -> fan on
clear -> keep fan on for 7 minutes
```

During quiet hours, occupancy is ignored. If midnight arrives while the fan is
running only because of occupancy or post-occupancy drying, the fan is turned
off. Humidity demand keeps the fan running until the corresponding derived
humidity status returns to `normal`.

Fan runtime guard defaults:

```text
minimum_on_time = 2 minutes
minimum_off_time = 1 minute
```

Humidity publisher guard defaults:

```text
humidity_minimum_run = 10 minutes
humidity_maximum_run = 90 minutes
```

After 90 minutes, a humidity publisher returns to `normal` unless the bathroom
is still extremely humid (`RH > 80%`). This prevents a bad sensor or unusual
weather condition from running the fan indefinitely.

## Entity configuration

Required environment variables:

```sh
export HOME_ASSISTANT_URL='http://homeassistant.local:8123'
export HOME_ASSISTANT_TOKEN='...'

export HAUTO_EXHAUST_FAN='switch.bathroom_exhaust_fan'

export HAUTO_AMBIENT_TEMP='sensor.hall_temperature'
export HAUTO_AMBIENT_HUMIDITY='sensor.hall_humidity'

export HAUTO_BATHROOM_1_TEMP='sensor.main_bathroom_temperature'
export HAUTO_BATHROOM_1_HUMIDITY='sensor.main_bathroom_humidity'
export HAUTO_BATHROOM_1_OCCUPANCY='binary_sensor.main_bathroom_occupancy'
export HAUTO_BATHROOM_1_HUMIDITY_STATUS='sensor.bathroom_1_excess_humidity'

export HAUTO_BATHROOM_2_TEMP='sensor.ensuite_temperature'
export HAUTO_BATHROOM_2_HUMIDITY='sensor.ensuite_humidity'
export HAUTO_BATHROOM_2_OCCUPANCY='binary_sensor.ensuite_occupancy'
export HAUTO_BATHROOM_2_HUMIDITY_STATUS='sensor.bathroom_2_excess_humidity'
```

Optional quiet-hour overrides:

```sh
export HAUTO_QUIET_START_MINUTE=0
export HAUTO_QUIET_END_MINUTE=480
export HAUTO_LOCAL_UTC_OFFSET_MINUTES=600
```

The example defaults to quiet hours from midnight to 08:00. It uses a fixed UTC
offset from `HAUTO_LOCAL_UTC_OFFSET_MINUTES` to calculate local time without
adding a time-zone dependency.

Run with:

```sh
cargo run --example bathroom_exhaust_fan
```

`HAUTO_EXHAUST_FAN` must identify a `switch.*` entity.
