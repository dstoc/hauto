# Proposal: Availability-aware sensor values

## Motivation

`hauto` now has typed waits and expectations for sensors:

```rust
let power = Sensor::<f64>::new("sensor.laundry_washing_machine_power")?;

power
    .expect_matching(&ctx, |value| *value < idle_below)
    .for_at_least(idle_delay)
    .await?;
```

That works well for sensors whose state is always a valid number. It is less
useful for Home Assistant sensors that can temporarily report `unknown` or
`unavailable`.

The concrete pressure point is
`contrib/appliance_power_status/appliance_power_status.rs`. The appliance
status automation wants this behavior:

* numeric power values are normal readings;
* `unknown`, `unavailable`, or empty state means "unknown";
* unknown-like states should interrupt a pending `idle` or `off` hold;
* malformed numeric states should still be visible as errors, not silently
  treated as valid readings.

Today the contrib sketch uses raw `EntityState` parsing and a local
`hold_power_below` helper so it can model `Option<f64>` manually. That is more
verbose than the typed wait API and it duplicates logic the framework already
has for held predicates.

## Problem statement

Current sensor decoding is intentionally strict:

* `Sensor<f64>` parses `EntityState.state` as `f64`.
* If the state is `unknown`, `unavailable`, empty, or non-numeric, decoding
  returns `Error::InvalidState`.
* `Sensor<String>` returns the raw state string and leaves interpretation to
  the caller.

This strict behavior should remain available. Some automations should fail when
a numeric sensor stops being numeric.

However, other automations need availability to be a normal part of the typed
state. For those automations, `unknown` and `unavailable` should be values that
can be matched in predicates and can interrupt `.for_at_least(...)` holds
without failing the automation.

Binary-like entities do not have the same immediate gap because
`BinaryState` already includes `Unknown` and `Unavailable`.

## Proposal

Add a sensor-specific availability wrapper:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum SensorValue<T> {
    Value(T),
    Unknown,
    Unavailable,
}
```

Implement it first for numeric sensors:

```rust
let power = Sensor::<SensorValue<f64>>::new("sensor.laundry_washing_machine_power")?;
```

This preserves the existing strict API while adding an explicit tolerant API:

```rust
Sensor::<f64>                 // strict numeric sensor
Sensor::<SensorValue<f64>>    // availability-aware numeric sensor
```

### Decoding semantics

For `Sensor<SensorValue<f64>>`:

```text
"12.3"        -> SensorValue::Value(12.3)
"unknown"     -> SensorValue::Unknown
"unavailable" -> SensorValue::Unavailable
""            -> SensorValue::Unknown
"abc"         -> Error::InvalidState
```

The empty-string mapping is included for practical migration compatibility with
AppDaemon-style helpers that commonly treat empty state as unknown-like. It is
unlikely to be a meaningful numeric sensor value.

Malformed non-empty, non-sentinel strings should remain errors. This keeps
integration bugs visible and avoids hiding bad data behind `Unknown`.

If the entity is missing from the cache, existing `read` semantics remain:

```rust
power.read(cache)? == None
```

If a state-change event deletes the entity while a wait or expectation is
running, existing entity wait behavior should continue to return
`Error::EntityNotFound`. A deleted entity is not a `SensorValue`; it is the
absence of an entity state. Automations that want deletion to mean `unknown`
should handle that separately through `read()` returning `None` or through a
state-change stream.

### Helper methods

Add small convenience methods:

```rust
impl<T> SensorValue<T> {
    pub fn as_value(&self) -> Option<&T>;
    pub fn into_value(self) -> Option<T>;
    pub fn is_value(&self) -> bool;
    pub fn is_unknown(&self) -> bool;
    pub fn is_unavailable(&self) -> bool;
}
```

This keeps predicates readable:

```rust
power
    .expect_matching(&ctx, move |value| {
        value.as_value().is_some_and(|power| *power < idle_below)
    })
    .for_at_least(idle_delay)
    .await?;
```

Unknown and unavailable values naturally interrupt the hold because the
predicate evaluates to `false`, returning `HoldResult::Interrupted { actual }`
instead of `Error::InvalidState`.

### Public API surface

Re-export `SensorValue` from `src/lib.rs`:

```rust
pub use state::SensorValue;
```

Add typed sensor support in `src/entity.rs`:

```rust
impl StateDecoder<SensorValue<f64>> for SensorValueF64Decoder {
    // ...
}

typed_readable_entity!(Sensor<SensorValue<f64>>, SensorValue<f64>, SensorValueF64Decoder);
sensor_state_entity!(SensorValue<f64>);
```

The exact decoder name is internal. The public surface should be the
`SensorValue<T>` type and the existing inherent methods that are already
generated for supported sensor states:

```rust
impl Sensor<SensorValue<f64>> {
    pub fn read(&self, cache: &StateCache<'_>) -> Result<Option<SensorValue<f64>>>;

    pub fn wait_until_matching<F>(
        &self,
        ctx: &Context,
        predicate: F,
    ) -> StateWait<'_, SensorValue<f64>>
    where
        F: Fn(&SensorValue<f64>) -> bool + Send + Sync + 'static;

    pub fn expect_matching<F>(
        &self,
        ctx: &Context,
        predicate: F,
    ) -> StateExpectation<'_, SensorValue<f64>>
    where
        F: Fn(&SensorValue<f64>) -> bool + Send + Sync + 'static;
}
```

Do not replace or weaken `Sensor<f64>`. Existing strict numeric behavior should
continue unchanged.

### First caller

After implementation, simplify
`contrib/appliance_power_status/appliance_power_status.rs` to use
`Sensor<SensorValue<f64>>` and the existing expectation builder instead of its
local raw-state hold helper.

The core delayed transition should become:

```rust
let held = matches!(
    power
        .expect_matching(&ctx, move |value| {
            value.as_value().is_some_and(|power| *power < idle_below)
        })
        .for_at_least(idle_delay)
        .await?,
    HoldResult::Held
);
```

The automation can still reclassify after every held result or interruption.
The important improvement is that availability is now represented in the typed
state instead of requiring manual raw event parsing.

The appliance sketch may still need a small `next_power_change` helper until
hauto has an entity-level `next_change` API. This proposal only removes the
need for a raw-state hold helper that reimplements `.for_at_least(...)`.

## Non-goals

* **Changing strict numeric sensors:** `Sensor<f64>` should continue returning
  `Error::InvalidState` for `unknown`, `unavailable`, empty, and malformed
  numeric strings.
* **Adding `SensorValue<String>` now:** String sensors already expose raw state
  through `Sensor<String>`, and string domains can legitimately use words like
  `unknown` as data. Add a string wrapper later only for a concrete use case.
* **Changing binary/light/switch state types:** `BinaryState` already includes
  `Unknown` and `Unavailable`; no new wrapper is needed for those entities in
  this proposal.
* **Modeling every Home Assistant sensor unit or device class:** This proposal
  only adds availability-aware decoding for numeric values.
* **Treating malformed numeric strings as availability:** Non-empty malformed
  numeric strings should still be errors.
* **Changing deletion semantics:** Entity deletion during a wait or expectation
  should continue to return `Error::EntityNotFound`.

## Suggested implementation shape

1. Add `SensorValue<T>` and helper methods in `src/state.rs`.
2. Re-export `SensorValue` from `src/lib.rs`.
3. Add an internal decoder for `SensorValue<f64>` in `src/entity.rs`.
4. Register `Sensor<SensorValue<f64>>` with the existing typed-readable sensor
   macro so it gets `read`, `wait_until_matching`, and `expect_matching`.
5. Add tests for decoding, reading from `StateCache`, waits, expectations, and
   hold interruption.
6. Update the appliance power status contrib sketch to use
   `Sensor<SensorValue<f64>>`.

## Verification

Verification should include:

```sh
CARGO_TARGET_DIR=/tmp/hauto-target cargo test
CARGO_TARGET_DIR=/tmp/hauto-target cargo clippy --all-targets -- -D warnings
CARGO_TARGET_DIR=/tmp/hauto-target cargo check --examples
cargo fmt --check
```

Specific test evidence should prove:

* `SensorValue::<f64>::Value` decodes from a valid numeric state.
* `unknown` decodes to `SensorValue::Unknown`.
* `unavailable` decodes to `SensorValue::Unavailable`.
* an empty string decodes to `SensorValue::Unknown`.
* a non-empty malformed numeric string returns `Error::InvalidState`.
* `Sensor<SensorValue<f64>>::read()` returns `Ok(Some(...))` for cached
  numeric and sentinel states.
* `Sensor<SensorValue<f64>>::read()` returns `Ok(None)` when the entity is not
  in the cache.
* `expect_matching(...).for_at_least(...)` over `SensorValue<f64>` returns
  `HoldResult::Interrupted` when the state changes from `Value(...)` to
  `Unknown` or `Unavailable` before the hold expires.
* The appliance power status contrib example compiles after switching away from
  raw hold parsing for sentinel sensor states.

## Success criteria

* Existing `Sensor<f64>` tests continue to pass unchanged.
* `Sensor<SensorValue<f64>>` supports `read`, `wait_until_matching`, and
  `expect_matching`.
* Unknown and unavailable numeric sensor states can be handled as normal values
  by typed predicates.
* The appliance power status sketch no longer needs a local raw-state
  `hold_power_below` helper.
