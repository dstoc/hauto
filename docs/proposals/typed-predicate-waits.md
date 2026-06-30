# Proposal: Typed Predicate Waits

## Motivation

`hauto` currently has a useful wait builder for binary sensors:

```rust
occupancy.wait_until_on(&ctx).await?;
occupancy
    .expect_off(&ctx)
    .for_at_least(Duration::from_secs(30))
    .await?;
```

This covers motion and occupancy automations well, but it does not generalize
to other common Home Assistant entities. A light, switch, or numeric sensor is
also a state source that users often want to wait on.

The concrete pressure point is `examples/temperature_threshold.rs`. It uses
`Sensor::<f64>::new(...)`, but then drops down to raw state changes and parses
`EntityState.state` manually:

```rust
let mut changes = ctx.state_changes(sensor.entity_id());
```

That example is acceptable as an escape hatch, but it shows the missing API:

```rust
temperature.wait_until_matching(&ctx, |value| *value > 30.0).await?;
```

The goal is to make waits feel like part of the typed entity layer rather than
a binary-sensor-only special case.

## Problem statement

The current implementation is concentrated in two places:

* `src/entity.rs` exposes `BinarySensor::wait_until`,
  `wait_until_on`, `wait_until_off`, `expect_state`, `expect_on`, and
  `expect_off`.
* `src/wait.rs` implements `StateWait` and `StateExpectation` around
  `BinaryState` only.

This has three practical limitations:

1. `Light` and `Switch` can be controlled through typed service methods, but
   there is no typed way to wait for their state to become `on` or `off`.
2. `Sensor<f64>` and `Sensor<String>` can be constructed, but callers must
   manually read `EntityState.state`, parse it, and subscribe to raw state
   changes.
3. The current wait builder semantics are valuable, but they are tied to
   `BinaryState` instead of a small reusable typed-state abstraction.

The framework should add typed waits without turning the entity layer into a
complete Home Assistant schema model.

## Proposal

Introduce a typed wait abstraction shared by the existing binary sensor waits
and new waits for lights, switches, and sensors.

The first user-facing addition should be predicate waits for sensors:

```rust
let temperature = Sensor::<f64>::new("sensor.office_temperature")?;

temperature
    .wait_until_matching(&ctx, |value| *value > 30.0)
    .for_at_least(Duration::from_secs(60))
    .within(Duration::from_secs(300))
    .await?;
```

Discrete entities should support equality waits and convenience aliases:

```rust
light.wait_until_on(&ctx).await?;
light.wait_until_off(&ctx).await?;

switch.wait_until_on(&ctx).await?;
switch.wait_until_off(&ctx).await?;
```

The existing binary sensor API should continue to work unchanged:

```rust
occupancy.wait_until_on(&ctx).require_transition().await?;
occupancy.expect_off(&ctx).for_at_least(duration).await?;
```

Expectations should generalize with the same typed-state layer:

```rust
light.expect_on(&ctx).for_at_least(Duration::from_secs(30)).await?;
switch.expect_off(&ctx).await?;

temperature
    .expect_matching(&ctx, |value| *value < 30.0)
    .for_at_least(Duration::from_secs(60))
    .await?;
```

### Typed state decoding

Add an internal trait that describes how an entity handle decodes its primary
state:

```rust
pub(crate) trait TypedStateEntity {
    type State: Clone + Send + Sync + 'static;

    fn entity_id(&self) -> &EntityId;
    fn decode_state(entity_id: &EntityId, raw: &EntityState) -> Result<Self::State>;
}
```

Initial implementations:

```text
BinarySensor -> BinaryState
Light        -> BinaryState
Switch       -> BinaryState
Sensor<f64>  -> f64
Sensor<String> -> String
```

`Light` and `Switch` can use `BinaryState` initially because Home Assistant's
common primary states for those domains are still `on`, `off`, `unknown`, and
`unavailable`. A later proposal can introduce `LightState` or `SwitchState` if
there is a concrete need for a distinct type.

`Sensor<String>` should expose the raw state string as its decoded value.

`Sensor<f64>` should parse the raw state string as `f64`. If the entity state
is non-numeric, including `unknown` or `unavailable`, decoding should return
`Error::InvalidState`. This is deliberately simple for the first version and
matches the current manual parsing in `examples/temperature_threshold.rs`.

### Wait builder API

Keep the existing `StateWait` and `TimedStateWait` shape, but generalize the
condition being evaluated.

The public API should expose two concepts:

```rust
entity.wait_until(&ctx, target)
entity.wait_until_matching(&ctx, predicate)
```

`wait_until` should be available where the decoded state supports equality:

```rust
impl Light {
    pub fn wait_until(&self, ctx: &Context, target: BinaryState) -> StateWait<'_, BinaryState>;
    pub fn wait_until_on(&self, ctx: &Context) -> StateWait<'_, BinaryState>;
    pub fn wait_until_off(&self, ctx: &Context) -> StateWait<'_, BinaryState>;
}
```

`wait_until_matching` should be available for typed entities:

```rust
impl Sensor<f64> {
    pub fn wait_until_matching<F>(
        &self,
        ctx: &Context,
        predicate: F,
    ) -> StateWait<'_, f64>
    where
        F: Fn(&f64) -> bool + Send + Sync + 'static;
}
```

The exact internal builder type may need to carry an erased predicate:

```rust
struct StateWait<'a, T> {
    ctx: &'a Context,
    entity_id: EntityId,
    condition: StateCondition<T>,
    require_transition: bool,
    hold_for: Option<Duration>,
}
```

The crate root should continue to re-export the user-facing wait types. If the
generic type parameter is exposed publicly, it should be documented through the
entity methods rather than requiring users to name it in normal code.

### Expectation builder API

Generalize `StateExpectation` in the same way as `StateWait`.

The public API should expose two matching expectation concepts:

```rust
entity.expect_state(&ctx, target)
entity.expect_matching(&ctx, predicate)
```

`expect_state` should be available where the decoded state supports equality:

```rust
impl Light {
    pub fn expect_state(
        &self,
        ctx: &Context,
        target: BinaryState,
    ) -> StateExpectation<'_, BinaryState>;

    pub fn expect_on(&self, ctx: &Context) -> StateExpectation<'_, BinaryState>;
    pub fn expect_off(&self, ctx: &Context) -> StateExpectation<'_, BinaryState>;
}
```

`expect_matching` should be available for typed entities:

```rust
impl Sensor<f64> {
    pub fn expect_matching<F>(
        &self,
        ctx: &Context,
        predicate: F,
    ) -> StateExpectation<'_, f64>
    where
        F: Fn(&f64) -> bool + Send + Sync + 'static;
}
```

Expectation semantics should stay distinct from wait semantics:

* `expect_matching` checks the current cached state immediately;
* if the current state does not satisfy the predicate, it returns
  `HoldResult::NotSatisfied { actual }`;
* if no hold duration is configured and the current state satisfies the
  predicate, it returns `HoldResult::Held`;
* if `.for_at_least(duration)` is configured, the predicate must stay true for
  that duration;
* if the predicate becomes false during the hold, it returns
  `HoldResult::Interrupted { actual }`;
* expectations do not have `require_transition()` or `within()` because they
  are assertions about current state, not waits for future state.

This gives the API a coherent split:

```text
wait_until_matching  = eventually become true
expect_matching      = must be true now, and optionally remain true
```

### Predicate semantics

Predicate waits should use the same lifecycle semantics as the existing
binary wait builder:

```text
wait starts
subscribe to matching state changes
check current cached state
complete immediately if the condition is already true and require_transition is not set
if for_at_least is set, start timing when the condition is true
reset the hold timer when the condition becomes false
within bounds the complete condition
connection loss cancels the context
lagged streams return an error
entity deletion returns EntityNotFound
```

`require_transition()` for predicate waits means:

* if the predicate is initially false, the wait can complete when it next
  becomes true;
* if the predicate is initially true, the wait must observe it become false
  and then true again.

This mirrors the current binary-sensor meaning.

`for_at_least(Duration::ZERO)` and `within(Duration::ZERO)` should preserve the
current edge-case behavior:

* zero hold completes once the condition is acquired;
* zero timeout allows an immediately satisfied condition to win, otherwise it
  returns `TimedOut`;
* a condition observed at or before its deadline wins over the timeout.

### Expectations

The current `expect_state(...).for_at_least(...)` API answers: "the entity must
be in this state now and must remain there." That remains useful for binary
sensors, lights, and switches.

Predicate expectations are the same primitive applied to a typed predicate:

```rust
temperature.expect_matching(&ctx, |v| *v > 30.0)
```

This should mean "the predicate must be true now." Adding
`.for_at_least(duration)` extends that to "the predicate must be true now and
must remain true for the duration."

### Fallback and error behavior

Typed waits and expectations should not hide Home Assistant's dynamic states.

For the initial implementation:

* missing entity at wait start behaves like the current binary wait: the wait
  subscribes and waits for a future state unless a deletion event is observed
  for an existing entity;
* missing entity at expectation start returns `EntityNotFound`, matching the
  current binary expectation behavior;
* entity deletion during a pending wait returns `EntityNotFound`;
* entity deletion during an expectation hold returns `EntityNotFound`;
* stream lag returns `Error::EventStream(EventStreamError::Lagged { .. })`;
* connection loss returns cancellation through the context;
* decode failure returns `InvalidState` and ends the wait or expectation.

For `Sensor<f64>`, `unknown`, `unavailable`, and other non-numeric strings are
decode failures in the first version. A future `SensorState<T>` wrapper can
model those values explicitly if needed.

### Examples

After this API exists, `examples/temperature_threshold.rs` can be simplified
or a new example can be added:

```rust
let temperature = Sensor::<f64>::new(required_env("HAUTO_TEMPERATURE_SENSOR")?)?;
let light = Light::new(required_env("HAUTO_LIGHT")?)?;
let threshold = required_env("HAUTO_THRESHOLD")?.parse::<f64>()?;

App::new(home_assistant_url, home_assistant_token)
    .automation_fn("temperature alert", move |ctx| {
        let temperature = temperature.clone();
        let light = light.clone();

        async move {
            loop {
                temperature
                    .wait_until_matching(&ctx, move |value| *value >= threshold)
                    .await?;

                light.turn_on(&ctx, LightTurnOn::default()).await?;

                temperature
                    .wait_until_matching(&ctx, move |value| *value < threshold)
                    .await?;

                light.turn_off(&ctx, LightTurnOff::default()).await?;
            }
        }
    })
    .run()
    .await?;
```

## Non-goals

This proposal does not attempt to:

* statically model every Home Assistant domain state;
* add domain-specific sensor wrappers such as `TemperatureSensor`;
* add a full `SensorState<T>` availability model in the first version;
* change service-call APIs;
* change the raw event stream API;
* change reconnection or generation lifecycle behavior.

## Suggested implementation shape

1. Add typed-state decoding helpers in `src/entity.rs` or a new
   `src/typed_state.rs`.
2. Refactor `src/wait.rs` so the wait and expectation state machines evaluate
   a reusable condition over decoded states instead of hard-coding
   `BinaryState`.
3. Preserve the existing `BinarySensor` methods by delegating them to the
   generic wait implementation.
4. Add `Light::wait_until_on/off` and `Switch::wait_until_on/off`.
5. Add `Light::expect_on/off` and `Switch::expect_on/off`.
6. Add `Sensor<f64>::wait_until_matching`,
   `Sensor<String>::wait_until_matching`, `Sensor<f64>::expect_matching`, and
   `Sensor<String>::expect_matching`.
7. Add tests in `src/tests.rs` for:
   * binary sensor compatibility;
   * light/switch on/off waits;
   * light/switch on/off expectations;
   * `Sensor<f64>` predicate success;
   * `Sensor<f64>` predicate `for_at_least` reset;
   * `Sensor<f64>` predicate expectation success;
   * `Sensor<f64>` predicate expectation interruption;
   * `within` timeout on a predicate;
   * deletion and lag behavior if the existing test helpers can trigger them;
   * non-numeric `Sensor<f64>` state returning `InvalidState`.
8. Update or add an example that uses
   `Sensor::<f64>::wait_until_matching`.

The first implementation should avoid a large trait-heavy public surface.
Private traits and helpers are fine; public methods on existing entity handles
are enough.

## Verification

Verification should include observable checks:

```sh
CARGO_TARGET_DIR=/tmp/hauto-target cargo test
CARGO_TARGET_DIR=/tmp/hauto-target cargo clippy --all-targets -- -D warnings
CARGO_TARGET_DIR=/tmp/hauto-target cargo check --examples
```

Specific test evidence should prove:

* all existing binary sensor wait tests still pass unchanged;
* `Light::wait_until_on` completes from cached `on` state;
* `Switch::wait_until_off` completes after a matching state-change event;
* `Light::expect_on().for_at_least(...)` returns `Held` when the light stays
  on;
* `Switch::expect_off()` returns `NotSatisfied` when the switch is currently
  on;
* `Sensor::<f64>::wait_until_matching(|v| *v > 30.0)` completes after a
  numeric state change;
* `Sensor::<f64>::expect_matching(|v| *v > 30.0)` returns `Held` from a
  matching cached numeric state;
* `Sensor::<f64>::expect_matching(...).for_at_least(...)` returns
  `Interrupted` when a later numeric state stops matching;
* predicate `require_transition()` requires true -> false -> true when the
  predicate is initially true;
* predicate `for_at_least` resets when the predicate becomes false;
* predicate `within` returns `WaitResult::TimedOut` as a normal outcome;
* non-numeric `Sensor<f64>` state returns `InvalidState`.

## Success criteria

* Existing `BinarySensor` wait and expectation public APIs remain source
  compatible.
* `Light` and `Switch` expose `wait_until_on` and `wait_until_off`.
* `Light` and `Switch` expose `expect_on` and `expect_off`.
* `Sensor<f64>` exposes `wait_until_matching` and `expect_matching`.
* Predicate waits support `require_transition`, `for_at_least`, and `within`
  with the same timing semantics as current binary waits.
* Predicate expectations support `for_at_least` with the same hold semantics
  as current binary expectations.
* `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo check --examples` pass.
* At least one example demonstrates a predicate wait over `Sensor<f64>`.
