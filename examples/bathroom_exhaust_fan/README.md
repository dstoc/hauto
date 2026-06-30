# Bathroom exhaust fan

This example controls one shared exhaust fan for two bathrooms.

Humidity is the primary safety trigger. Occupancy is a comfort/odour trigger
only outside quiet hours, so a normal overnight bathroom visit does not start
the fan, but an overnight shower still does.

```text
fan_on = bathroom_1_demand OR bathroom_2_demand
```

Each bathroom has independent state. The shared fan runs when either bathroom
needs it.

## Demand model

Each bathroom can demand the fan for humidity or occupancy:

```text
bathroom_demand = humidity_demand OR daytime_occupancy_demand
```

During quiet hours, occupancy is ignored:

```text
00:00 <= local time < 08:00:
    bathroom_demand = humidity_demand only
```

The example defaults to quiet hours from midnight to 08:00. It uses a fixed UTC
offset from `HAUTO_LOCAL_UTC_OFFSET_MINUTES` to calculate local time without
adding a time-zone dependency.

## Humidity model

The example compares each bathroom against a nearby ambient room, using
absolute humidity where temperature is available:

```text
bathroom_absolute_humidity = f(bathroom_temp, bathroom_rh)
ambient_absolute_humidity  = f(ambient_temp, ambient_rh)
humidity_excess = bathroom_absolute_humidity - ambient_absolute_humidity
```

Humidity demand starts when any of these are true:

```text
absolute humidity excess > 2.0 g/m³
OR bathroom RH > ambient RH + 12%
OR bathroom RH > 70%
OR bathroom RH rises by > 6% over 5 minutes
OR bathroom absolute humidity rises by > 0.6 g/m³ over 5 minutes
```

Humidity demand clears only after the bathroom has dried back down for 5
continuous minutes:

```text
absolute humidity excess < 0.8 g/m³
AND bathroom RH < ambient RH + 6%
AND bathroom RH < 65%
```

The start and clear thresholds are deliberately different. That hysteresis
keeps the fan from flapping around a single threshold.

Possible improvement: the example records rate-of-rise samples whenever the
automation wakes for a relevant state change or poll tick, so sample spacing is
not perfectly constant. For sharper shower detection, keep a fixed-cadence
humidity history, such as one sample every 30 seconds, and use sensor-change
events only to wake the automation for immediate threshold re-evaluation.

## Occupancy model

Outside quiet hours:

```text
occupied -> fan on
clear -> keep fan on for 7 minutes
```

During quiet hours, occupancy is ignored. If midnight arrives while the fan is
running only because of occupancy or post-occupancy drying, the fan is turned
off. Humidity demand keeps the fan running until humidity clears or the maximum
runtime guard applies.

## Runtime guards

Defaults:

```text
minimum_on_time = 2 minutes
minimum_off_time = 1 minute
humidity_minimum_run = 10 minutes
humidity_maximum_run = 90 minutes
```

When humidity demand starts, the fan runs for at least 10 minutes. After that,
it keeps running until the clear condition holds. After 90 minutes, it stops
unless the bathroom is still extremely humid (`RH > 80%`).

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

export HAUTO_BATHROOM_2_TEMP='sensor.ensuite_temperature'
export HAUTO_BATHROOM_2_HUMIDITY='sensor.ensuite_humidity'
export HAUTO_BATHROOM_2_OCCUPANCY='binary_sensor.ensuite_occupancy'
```

Optional quiet-hour overrides:

```sh
export HAUTO_QUIET_START_MINUTE=0
export HAUTO_QUIET_END_MINUTE=480
export HAUTO_LOCAL_UTC_OFFSET_MINUTES=600
```

Run with:

```sh
cargo run --example bathroom_exhaust_fan
```

`HAUTO_EXHAUST_FAN` may be any entity domain with `turn_on` and `turn_off`
services, such as `switch.*` or `fan.*`.
