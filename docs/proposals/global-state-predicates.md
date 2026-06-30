# Proposal: Global State Predicates

## Motivation

`hauto` currently provides excellent primitives for waiting on single entity
state changes through `wait_until_matching`. However, users often need to wait
for multiple states to be true *simultaneously* (e.g., waiting until the
temperature is over 20.0 AND the humidity is under 50.0).

Currently, this requires manual synchronization using `tokio::try_join!` and
looping with `expect_matching`:

```rust
loop {
    tokio::try_join!(
        temperature.wait_until_matching(&ctx, |v| *v > 20.0),
        humidity.wait_until_matching(&ctx, |v| *v < 50.0),
    )?;

    if matches!(temperature.expect_matching(&ctx, |v| *v > 20.0).await?, HoldResult::Held) &&
       matches!(humidity.expect_matching(&ctx, |v| *v < 50.0).await?, HoldResult::Held) {
        break;
    }
}
```

This is verbose, error-prone, and slightly inefficient since the futures are
dropped and re-created if the state toggles back and forth.

## Problem statement

There is currently no ergonomic way to evaluate a combined condition across
multiple entities simultaneously because:

1. The Home Assistant state cache in `GenerationState` is private.
2. `Context` exposes filtered state change streams for individual entities,
   but not a public global wait primitive over all state changes.
3. Entities like `Sensor<f64>` and `Light` do not expose synchronous read
   methods to extract their typed state from a cache snapshot.

## Proposal

Introduce a global state wait mechanism that evaluates a user-provided closure
against a read-only state cache every time any entity in Home Assistant
changes.

```rust
ctx.wait_until_state(|state| {
    let Some(t) = temperature.read(state)? else {
        return Ok(false);
    };
    let Some(h) = humidity.read(state)? else {
        return Ok(false);
    };

    Ok(t > 20.0 && h < 50.0)
})
.for_at_least(Duration::from_secs(300))
.within(Duration::from_secs(1800))
.await?;
```

### StateCache abstraction

Introduce a public `StateCache` struct that provides read-only access to the
internal `GenerationState` cache:

```rust
pub struct StateCache<'a> {
    generation: &'a GenerationState,
}
```

This prevents users from modifying the cache while giving them a synchronous
snapshot of the current state. `StateCache` should live in a cache/state module
such as `src/cache.rs` or `src/state_cache.rs`, not in `src/wait.rs`, because
it is a general state-reading abstraction rather than only a wait detail.

### Entity Read Methods

Expose public `read` methods on entities that decode their typed state from
the cache. These methods should internally use the existing `pub(crate) trait
TypedStateEntity`.

Only supported typed entities should expose `read`. Do not add a blanket
`impl<T> Sensor<T>` method, because the framework currently only knows how to
decode specific sensor state types.

```rust
impl Sensor<f64> {
    pub fn read(&self, cache: &StateCache) -> Result<Option<f64>>;
}

impl Sensor<String> {
    pub fn read(&self, cache: &StateCache) -> Result<Option<String>>;
}

impl Light {
    pub fn read(&self, cache: &StateCache) -> Result<Option<BinaryState>>;
}
```

Initial read methods:

```text
BinarySensor   -> Result<Option<BinaryState>>
Light          -> Result<Option<BinaryState>>
Switch         -> Result<Option<BinaryState>>
Sensor<f64>    -> Result<Option<f64>>
Sensor<String> -> Result<Option<String>>
```

### Wait Builder API

Add `wait_until_state` to `Context`:

```rust
impl Context {
    pub fn wait_until_state<F>(&self, predicate: F) -> GlobalStateWait<'_, F>
    where
        F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
    {
        // ...
    }
}
```

`GlobalStateWait` should expose `.for_at_least(duration)` and
`.within(duration)` builder methods, matching the existing entity wait shape.

Without `.within(...)`, awaiting the builder returns `Result<()>`.

With `.within(...)`, awaiting the builder returns `Result<WaitResult>`, where
timeout is a normal `Ok(WaitResult::TimedOut)` outcome.

`GlobalStateWait` should evaluate as follows:

1. Ensure the current generation is active.
2. Subscribe to the global `generation.state_changes` broadcast channel.
3. Evaluate the predicate against the current cache.
4. If false, wait for the next global state change event.
5. On every global state change event, re-evaluate the predicate.
6. If `for_at_least(duration)` is configured, start a timer when the predicate
   becomes true. If the predicate becomes false before the timer expires, reset
   the wait.
7. Complete when the predicate returns `Ok(true)` (and the hold duration, if
   any, is satisfied).
8. Return early on `Err(_)`.

Subscribing before the initial cache evaluation avoids missing a state change
between reading the cache and subscribing to future changes.

Do not add `require_transition()` in the first version. For global predicates,
transition semantics are less obvious because any combination of entities can
make the predicate flip. Add this later only if a concrete use case needs it.

Global waits intentionally run the predicate after any Home Assistant state
change. This is simple and correct, but less efficient than entity-specific
waits in busy Home Assistant instances. Predicate closures should be cheap,
synchronous, and non-blocking; they must not perform async work.

### Fallback and error behavior

* If the predicate returns `Ok(false)`, the wait continues.
* If an entity is not found in the cache, `read()` returns `Ok(None)`. The
  user's predicate must decide whether this means the condition is unmet or if
  it should bubble up an error.
* If an entity's state cannot be decoded (e.g. invalid string for `f64`),
  `read()` returns `Err(Error::InvalidState)`. If the predicate uses `?`, this
  will abort the global wait.
* Connection loss cancels the wait through the `Context` lifecycle as with
  existing waits.
* Broadcast lag should return `Error::EventStream(EventStreamError::Lagged {
  .. })`, matching existing state streams.

## Non-goals

* **Building a Template Engine:** We are not building a DSL or stringly-typed
  template evaluation engine like native Home Assistant. Standard Rust
  closures are sufficient.
* **Deprecating single-entity waits:** `entity.wait_until_matching` remains the
  preferred, more efficient approach for single entities.
* **Scoped global subscriptions:** This proposal does not add
  `wait_until_state_on([entity_ids], ...)`. A scoped optimization can be added
  later if global predicates prove too noisy in practice.

## Suggested implementation shape

1. Define `pub struct StateCache<'a>` in `src/cache.rs` or
   `src/state_cache.rs`, and re-export it from `src/lib.rs`.
2. Add `pub fn read` to `BinarySensor`, `Light`, `Switch`, `Sensor<f64>`, and
   `Sensor<String>` in `src/entity.rs`.
3. Add `Context::wait_until_state` in `src/context.rs`.
4. Implement `GlobalStateWait` in `src/wait.rs` that polls
   `ctx.home_assistant.generation.state_changes.subscribe()`.

## Example

Add a new `examples/global_wait.rs` instead of rewriting an existing
single-entity example. The current examples are intentionally focused on
single-entity waits, raw event streams, service calls, state publishing, or
timer cancellation; `wait_until_state` is most useful when a condition spans
multiple entities.

The example should read these environment variables:

* `HOME_ASSISTANT_URL`
* `HOME_ASSISTANT_TOKEN`
* `HAUTO_TEMPERATURE_SENSOR`
* `HAUTO_HUMIDITY_SENSOR`
* `HAUTO_TEMPERATURE_THRESHOLD`
* `HAUTO_HUMIDITY_THRESHOLD`
* `HAUTO_LIGHT`

It should wait until both numeric sensor thresholds are satisfied at the same
time, hold that combined condition for a short duration, and then turn on the
configured light:

```rust
ctx.wait_until_state(|state| {
    let Some(temperature) = temperature.read(state)? else {
        return Ok(false);
    };
    let Some(humidity) = humidity.read(state)? else {
        return Ok(false);
    };

    Ok(temperature >= temperature_threshold && humidity <= humidity_threshold)
})
.for_at_least(Duration::from_secs(300))
.await?;
```

## Verification

Verification should include observable checks:

```sh
CARGO_TARGET_DIR=/tmp/hauto-target cargo test
CARGO_TARGET_DIR=/tmp/hauto-target cargo clippy --all-targets -- -D warnings
```

Specific test evidence should prove:
* `Sensor<f64>::read()` returns `Ok(Some(value))` when the cache has a valid
  numeric state.
* `Sensor<f64>::read()` returns `Ok(None)` when the entity is missing.
* `Sensor<f64>::read()` returns `Err(InvalidState)` when the cache contains a
  non-numeric string.
* `wait_until_state` returns immediately if the condition is already satisfied
  in the initial cache state.
* `wait_until_state` wakes up and evaluates when an unrelated entity changes
  state, but only completes when the predicate entities change and satisfy the
  condition.
* `wait_until_state(...).for_at_least(...)` correctly resets its hold timer if
  the predicate becomes false during the wait window.
* `wait_until_state(...).within(...)` returns `WaitResult::TimedOut` as a
  normal outcome.
* `examples/global_wait.rs` compiles and demonstrates a compound predicate
  over two `Sensor<f64>` handles.

## Success criteria

* `StateCache` is publicly available and read-only.
* All existing typed entities expose a `read(&StateCache)` method.
* `Context::wait_until_state` accepts a predicate and successfully waits for
  compound conditions.
* A new `examples/global_wait.rs` successfully demonstrates waiting for two
  sensors to cross thresholds simultaneously.
