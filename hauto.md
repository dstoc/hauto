# Proposal: A Rust Automation Framework for Home Assistant

## Motivation

Home Assistant automations are powerful, but complex stateful behavior can become awkward when expressed in YAML. Timer-heavy automations, cancellation logic, multi-step state machines, reusable behavior, and domain-specific semantics are often easier to express in code.

AppDaemon addresses this by providing a Python runtime for Home Assistant automations. This proposal explores a Rust equivalent: a small, typed, event-driven automation framework that connects to Home Assistant, subscribes to state changes, calls services, and exposes a pleasant Rust API for writing personal automations.

The goal is not to statically model all of Home Assistant. Home Assistant is intentionally dynamic, supports custom integrations, and exposes service schemas that may vary by integration, device, and version. Instead, the framework should provide strong ergonomics for common personal automation cases while retaining an escape hatch to raw Home Assistant APIs.

## Problem statement

Home Assistant’s native automation model is less ergonomic for some classes of logic:

* Timers that must be started, cancelled, and replaced.
* State machines that span multiple events.
* Reusable automation patterns across rooms or devices.
* Automations where entity semantics matter, such as “occupied”, “open”, “running”, or “clear”.
* Logic that benefits from normal programming-language abstractions, tests, and type checking.

Existing Rust Home Assistant clients provide useful API access, but do not provide an AppDaemon-like automation runtime. A Rust automation framework should fill that gap without trying to become a complete reimplementation of Home Assistant’s automation system.

## Proposal

Build a Rust framework for Home Assistant automations with three layers:

1. A raw Home Assistant client layer.
2. A small typed entity and event layer.
3. A user-facing automation runtime.

The framework should be optimized for personal automation daemons, not for complete static typing of every Home Assistant integration.

### Naming

The project should use a name that suggests automation rather than a general Home Assistant client.

Possible names:

* `hauto`
* `hass-daemon`
* `hass-rs-daemon`
* `ha-rules`
* `homekitten`
* `rustdaemon`

Recommended working name:

```text
hauto
```

Rationale:

* Short.
* Not tied to AppDaemon.
* Suggests “Home Assistant automation”.
* Suitable for both a library crate and a daemon binary.

The project should start as a single crate:

```text
hauto
```

Additional crates should only be introduced when a concrete implementation need
justifies them.

### Semantics

The framework should treat Home Assistant as a dynamic event source and service target.

Core primitives:

```text
read current state
subscribe to state changes
publish an ephemeral state
call services
sleep
apply a timeout to any future
spawn scoped child tasks
observe context cancellation
schedule and cancel delayed actions
decode common entity states
```

The user-facing API should avoid raw JSON in normal automation code, but raw JSON should remain available.

Example target style:

```rust
let occupancy = BinarySensor::new("binary_sensor.office_occupancy")?;
let light = Light::new("light.office")?;

app.automation_fn("office occupancy light", move |ctx| {
    let occupancy = occupancy.clone();
    let light = light.clone();

    async move {
        loop {
            occupancy
                .wait_until_on(&ctx)
                .require_transition()
                .await?;

            light.turn_on(&ctx, LightTurnOn {
                brightness_pct: Some(100),
                transition: Some(Duration::from_secs(1)),
                ..Default::default()
            }).await?;
        }
    }
});
```

More complex automations should support long-running state machines:

```rust
struct OccupancyLight {
    light: Light,
    occupancy: BinarySensor,
    dim_after: Duration,
    off_after: Duration,
}

impl Automation for OccupancyLight {
    async fn run(self, ctx: Context) -> Result<()> {
        loop {
            self.occupancy.wait_until_on(&ctx).await?;

            self.light.turn_on(&ctx, LightTurnOn {
                brightness_pct: Some(100),
                transition: Some(Duration::from_secs(1)),
                ..Default::default()
            }).await?;

            self.occupancy.wait_until_off(&ctx).await?;

            match self.occupancy
                .expect_off(&ctx)
                .for_at_least(self.dim_after)
                .await?
            {
                HoldResult::Held => {
                    self.light.turn_on(&ctx, LightTurnOn {
                        brightness_pct: Some(50),
                        transition: Some(Duration::from_secs(1)),
                        ..Default::default()
                    }).await?;
                }
                HoldResult::NotSatisfied { .. } | HoldResult::Interrupted { .. } => continue,
            }

            match self.occupancy
                .expect_off(&ctx)
                .for_at_least(self.off_after)
                .await?
            {
                HoldResult::Held => {
                    self.light.turn_off(&ctx, LightTurnOff {
                        transition: Some(Duration::from_secs(1)),
                        ..Default::default()
                    }).await?;
                }
                HoldResult::NotSatisfied { .. } | HoldResult::Interrupted { .. } => continue,
            }
        }
    }
}
```

The framework should not hide that Home Assistant is dynamic. Runtime errors such as unavailable entities, unsupported services, disconnected WebSockets, and invalid service payloads should remain explicit.

### Context and task lifecycle

Each automation should receive a `Context` representing its runtime
capabilities and cancellation scope. When the automation exits or `App` shuts
down, the context is cancelled. Losing the Home Assistant connection also
cancels every context created for that connection.

Automation code should be able to observe cancellation directly:

```rust
fn cancelled(&self) -> impl Future<Output = ()> + '_;
```

`ctx.cancelled()` completes once and remains ready after cancellation. It is
primarily useful when a root automation supervises scoped child tasks:

```rust
ctx.spawn(event_loop);
ctx.spawn(timer_loop);

ctx.cancelled().await;
Ok(())
```

Child work started through `ctx.spawn(...)` should belong to that scope and
return an awaitable task handle. Dropping the handle should not detach the
task; the context continues to own it. Child failures observed through the
handle are returned to the caller, while unobserved child failures are surfaced
by the runtime.

When a scope ends, the runtime should first signal cancellation, wake
context-aware helpers, and allow the root and child tasks to exit
cooperatively. It must then abort and join any tasks that do not finish; a
non-cooperative automation must never block shutdown or reconnection
indefinitely. Futures returned by state, timer, and service helpers should be
cancellation-safe when dropped. Helpers blocked at cancellation should return
a cancellation error, which the runtime treats as normal lifecycle completion.

An automation's `run` future is invoked once per connection generation. Most
reactive automations should contain an explicit loop. Returning `Ok(())` ends
the automation for the rest of that generation; the runtime must not
automatically invoke it again, because doing so could create a busy loop.
Returning another error also ends that instance and surfaces the failure.

### Connection lifecycle

`App` should manage the Home Assistant connection in generations. Each
generation should bootstrap its state cache without leaving a race between the
initial snapshot and live events:

1. Connect and authenticate.
2. Subscribe to all `state_changed` events and wait for confirmation.
3. Buffer incoming state changes.
4. Request the complete state snapshot.
5. Build the cache from the snapshot and replay buffered changes in order.
6. Switch the subscription to live cache updates and event fan-out.
7. Start registered automations with fresh contexts for that generation.

Replaying all buffered changes is safe even when the snapshot already includes
some of them, because ordered changes are applied as complete replacements for
their entity's cached state. This ordering prevents a state change from being
missed between snapshot retrieval and event subscription.

When the connection is lost, the runtime should:

1. Close event streams with `ConnectionLost` and fail pending operations.
2. Cancel every automation context from that generation.
3. Cancel and join scoped child tasks and timers.
4. Discard the old client and event streams.
5. Bootstrap a new connection generation and state cache.
6. Start fresh instances of all registered automations.

Socket EOF, read or write failure, protocol failure, and heartbeat failure
should all end the generation. Built-in helpers may observe either the terminal
stream error or context cancellation while teardown proceeds; both should map
to normal connection-generation cancellation rather than an automation
failure. Pending service calls still use the `NotSent`/`OutcomeUnknown`
distinction described below.

No automation future, event stream, timer, or `for_at_least()` progress should
survive across generations. Registration must therefore retain a restartable
automation function or factory rather than a one-shot future.

### Entity model

The entity model should be lightweight.

Base entity types:

```rust
EntityId
Entity
Light
Switch
BinarySensor
Sensor<T>
```

Entity handles should be cloneable values containing only an `EntityId` and
type information. Constructing one performs no I/O and does not require an
active `App`:

```rust
let occupancy = BinarySensor::new("binary_sensor.office_occupancy")?;
```

Constructors should validate entity-ID syntax and the expected domain locally.
They should return `InvalidEntityId` or `InvalidDomain`, but should not check
whether the entity currently exists.

Existence is checked against the current generation's state cache:

```rust
let state = occupancy.state(&ctx).await?;
```

`state()` should return `EntityNotFound` when the ID is absent. An entity whose
state is `unknown` or `unavailable` still exists and returns the corresponding
typed state. The MVP does not need a separate `exists()` helper because
`state()` provides the more useful distinction.

If a later `state_changed` event has `new_state: None`, the entity should be
removed from the cache. Pending typed waits for that entity should then return
`EntityNotFound`; they should not wait for an entity with the same ID to
reappear.

The MVP should expose binary sensors in terms of Home Assistant's `on` and
`off` states. Domain-specific wrappers can be considered later if repeated
automation code demonstrates that they provide enough value.

### State semantics

The framework should model common Home Assistant state values explicitly:

```rust
enum Availability {
    Available,
    Unavailable,
    Unknown,
}

enum BinaryState {
    On,
    Off,
    Unknown,
    Unavailable,
}

struct EntityState {
    entity_id: EntityId,
    state: String,
    attributes: serde_json::Map<String, serde_json::Value>,
    last_changed: String,
    last_updated: String,
}
```

The raw state type should preserve Home Assistant's state string, attributes,
and timestamps even when the typed layer does not interpret them. The
framework should avoid exposing only `bool`, because `unknown` and
`unavailable` are meaningful Home Assistant states.

### State publishing

The raw client should support Home Assistant's REST state endpoint for
publishing status entities:

```rust
struct StateWrite {
    state: String,
    attributes: serde_json::Value,
}

enum SetStateResult {
    Created(EntityState),
    Updated(EntityState),
}

enum DeleteStateResult {
    Deleted,
    NotFound,
}
```

Example:

```rust
let status_entity = EntityId::new("sensor.my_automation_status")?;

let result = ctx.home_assistant()
    .set_state_raw(
        &status_entity,
        StateWrite {
            state: status.to_string(),
            attributes: json!({
                "friendly_name": friendly_name,
                "icon": icons
                    .get(&status)
                    .cloned()
                    .unwrap_or_else(|| "mdi:help-circle".to_string()),
            }),
        },
    )
    .await?;
```

`attributes` must contain a JSON object. `set_state_raw` should use
`POST /api/states/<entity_id>`, returning `Created` for HTTP 201 and `Updated`
for HTTP 200. The operation is an upsert: it should not check existence first.

Ephemeral states should also support explicit deletion:

```rust
match ctx.home_assistant()
    .delete_state_raw(&status_entity)
    .await?
{
    DeleteStateResult::Deleted => { ... }
    DeleteStateResult::NotFound => { ... }
}
```

`delete_state_raw` should use `DELETE /api/states/<entity_id>`. Deletion must
be caller-controlled; the runtime should not automatically remove published
states when an automation ends, a connection generation restarts, or `App`
shuts down.

Entities created this way are ephemeral state-machine entries. They do not
create entity-registry or device-registry entries, do not control physical
devices, and do not persist across Home Assistant restarts. Publishing derived
automation status is compatible with the non-goal of storing authoritative
automation state in Home Assistant.

The returned `EntityState` is the immediate result of the REST request. The
runtime cache should continue to update only from the ordered
`state_changed` stream; applying the REST response directly could overwrite a
newer concurrent event. A cache read immediately after the REST response may
therefore briefly observe the previous value.

Like service calls, a state write that may have reached Home Assistant before
the HTTP response was lost should fail with `OutcomeUnknown` and must not be
retried automatically. The same rule applies to deletion, although callers may
choose to retry deletion because it is idempotent.

The MVP should not expose a generic raw REST request API. Additional REST
operations should be added explicitly when a concrete automation requires
them.

### Service semantics

Common services should have typed request structs.

For lights:

```rust
struct LightTurnOn {
    brightness_pct: Option<u8>,
    brightness: Option<u8>,
    transition: Option<Duration>,
    color_temp_kelvin: Option<u16>,
    rgb_color: Option<(u8, u8, u8)>,
    effect: Option<String>,
}

struct LightTurnOff {
    transition: Option<Duration>,
}
```

The framework should validate obvious constraints before sending requests:

```text
brightness_pct must be 0..=100
brightness must be 0..=255
transition must be non-negative
rgb values must be 0..=255
```

It should not attempt to prove that a specific physical device supports every requested option. That remains a runtime Home Assistant/device concern.

All typed services should have a raw escape hatch:

```rust
ctx.home_assistant()
    .call_service_raw("light", "turn_on", json!({
        "entity_id": "light.office",
        "brightness_pct": 50,
        "transition": 1
    }))
    .await?;
```

Service delivery cannot provide exactly-once semantics across connection loss.
If the connection fails before a request is written, the call should return a
`NotSent` connection error. If it may have been written but no response was
received, it should return `OutcomeUnknown`. Home Assistant may have executed
the service in the latter case.

The runtime must never retry an `OutcomeUnknown` call automatically. The
automation may choose to inspect current state or retry an operation it knows
is idempotent.

### Event semantics

The shared runtime connection should subscribe to all `state_changed` events,
not to every event type on Home Assistant's event bus.

Initial event types:

```rust
enum EventStreamError {
    Lagged { dropped: Option<usize> },
    ConnectionLost,
}

struct StateChangedEvent {
    entity_id: EntityId,
    old_state: Option<EntityState>,
    new_state: Option<EntityState>,
}
```

The runtime should provide filtered streams:

```rust
ctx.state_changes(entity)
ctx.binary_sensor_changes(sensor)
ctx.light_changes(light)
```

The connection reader should update the state cache before fan-out and must
never await an individual consumer. For the MVP, each consumer should have a
bounded queue of 64 matching events. If that queue fills, the consumer should
receive a terminal `Lagged` error and be removed; events must not be silently
dropped. The dropped count may be `None` when it cannot be measured exactly.

Streams should preserve event order within a connection generation.
Connection loss should produce a terminal `ConnectionLost` error while the
runtime cancels the associated automation contexts. New streams are created
when automations restart in the next generation.

`StateWait` and `StateExpectation` should treat `Lagged` as an error and end
the current automation instance. They must not continue evaluating a
continuity-sensitive condition after losing history.

The typed layer should provide a configurable state-wait builder:

```rust
sensor.wait_until(&ctx, BinaryState::On).await?;

match sensor
    .wait_until(&ctx, BinaryState::On)
    .require_transition()
    .for_at_least(duration)
    .within(timeout)
    .await?
{
    WaitResult::Satisfied => { ... }
    WaitResult::TimedOut => { ... }
}
```

`wait_until` should return a `StateWait` builder implementing `IntoFuture`
rather than being an `async fn` directly. Awaiting the unmodified builder
produces `Result<()>`: it completes immediately when the current state equals
the target and otherwise waits for it. The current-state check and event
subscription must be race-free.

Builder modifiers should compose:

```text
require_transition()  require entering the target state after the wait begins
for_at_least(duration) require the target state continuously for the duration
within(duration)      bound the total time allowed for the complete condition
```

If the entity already has the target state, `require_transition()` requires it
to leave and subsequently re-enter that state. `for_at_least()` starts timing
when the target is observed; any other state resets that timer. It does not
infer earlier elapsed time from Home Assistant's `last_changed`. Connection
loss cancels the entire wait as part of cancelling its automation context.

`within()` includes any time spent waiting for the target and satisfying
`for_at_least()`, measured from when the builder is first awaited. It changes
the awaited result to:

```rust
enum WaitResult {
    Satisfied,
    TimedOut,
}
```

Timing out is a normal outcome rather than an error.

The builder types should make the output change from `within()` explicit:

```rust
fn wait_until<'a>(
    &'a self,
    ctx: &'a Context,
    target: BinaryState,
) -> StateWait<'a>;

impl<'a> StateWait<'a> {
    fn require_transition(self) -> Self;
    fn for_at_least(self, duration: Duration) -> Self;
    fn within(self, duration: Duration) -> TimedStateWait<'a>;
}

// IntoFuture::Output = Result<()>
struct StateWait<'a> { /* ... */ }

// IntoFuture::Output = Result<WaitResult>
struct TimedStateWait<'a> { /* ... */ }
```

Because binary `On` and `Off` waits are common, `BinarySensor` should provide
aliases that return the same builder and retain all modifiers:

```rust
sensor.wait_until_on(&ctx)
sensor.wait_until_off(&ctx)
```

Waiting for a state and requiring a state immediately are distinct operations.
The latter should use a `StateExpectation` builder:

```rust
match sensor
    .expect_state(&ctx, BinaryState::On)
    .for_at_least(duration)
    .await?
{
    HoldResult::Held => { ... }
    HoldResult::NotSatisfied { actual } => { ... }
    HoldResult::Interrupted { actual } => { ... }
}
```

`expect_state` checks the current state immediately. If it differs from the
target, it returns `NotSatisfied` without waiting. Once the target is
confirmed, `for_at_least()` observes it for the requested duration. Leaving
the target returns `Interrupted`; it does not reset and re-arm as
`StateWait::for_at_least()` does. Connection loss cancels the automation
context instead of producing a normal `HoldResult`.

The current-state check and observation setup must be race-free.

The result should preserve the state that interrupted the hold:

```rust
enum HoldResult<T> {
    Held,
    NotSatisfied { actual: T },
    Interrupted { actual: T },
}
```

The expectation builder should have a stable output type:

```rust
fn expect_state<'a>(
    &'a self,
    ctx: &'a Context,
    target: BinaryState,
) -> StateExpectation<'a>;

impl<'a> StateExpectation<'a> {
    fn for_at_least(self, duration: Duration) -> Self;
}

// IntoFuture::Output = Result<HoldResult<BinaryState>>
struct StateExpectation<'a> { /* ... */ }
```

Binary sensors should provide `expect_on()` and `expect_off()` aliases that
return the same `StateExpectation` builder:

```rust
sensor.expect_on(&ctx)
sensor.expect_off(&ctx)
```

State comparisons for these builders should use the decoded primary state
only; attribute-only updates do not count as transitions or interrupt a hold.
`for_at_least(Duration::ZERO)` should complete as soon as the target is
acquired. `within(Duration::ZERO)` should allow an immediately satisfied
condition to win but should otherwise return `TimedOut`. More generally, a
condition observed at or before its deadline wins over the timeout.

### Timer semantics

Timers are a major reason to use a code-based automation framework.

The fundamental timer should be a cancellation-safe sleep future:

```rust
ctx.sleep(Duration::from_secs(30)).await?;
```

A generic timeout helper should compose a duration with any fallible future:

```rust
enum TimeoutResult<T> {
    Completed(T),
    TimedOut,
}

match ctx.timeout(duration, operation()).await? {
    TimeoutResult::Completed(value) => { ... }
    TimeoutResult::TimedOut => { ... }
}
```

Timing out is a normal outcome rather than an error. Dropping the timeout
future should cancel both the timer and the future being timed.

For separately scheduled actions, `run_after` should be a convenience built
from scoped spawning and `sleep`:

```rust
let timer = ctx.run_after(Duration::from_secs(30), async move {
    light.turn_off(...).await
});

timer.cancel().await?;
```

The returned handle should be awaitable for completion and expose callback
errors. Cancellation should be idempotent and should complete only once the
callback can no longer start or has been stopped. Scheduled actions should be
cancelled automatically when their automation context ends. Dropping the timer
handle should not cancel the action; callers must call `cancel()` when they
want cancellation before the context ends.

### App construction

The calling program should provide the Home Assistant URL and access token
directly to `App`:

```rust
let app = App::new(home_assistant_url, access_token);
```

`home_assistant_url` should be the instance's `http` or `https` base URL.
`App` should derive the corresponding `/api/websocket` URL and REST endpoints
from it.

The framework should not load environment variables or files implicitly.
Calling programs can choose how to obtain these values before constructing
`App`. Automation behavior should remain Rust code registered with `App`.

### Error handling

The framework should distinguish between:

```text
connection errors
authentication errors
entity not found
invalid state decoding
service call rejected by Home Assistant
service call not sent
service call outcome unknown
state mutation outcome unknown
automation task failure
context cancellation
```

Automation task failures should be surfaced explicitly rather than silently
terminating the task. Cancellation caused by ending a connection generation is
normal lifecycle behavior rather than an automation failure.

### Non-goals

The project should not attempt to:

* Replace Home Assistant.
* Replace Home Assistant integrations.
* Provide dashboards.
* Discover devices independently.
* Fully model every Home Assistant service schema.
* Prove at compile time that a specific physical device supports a specific feature.
* Become a general visual automation editor.
* Provide complete compatibility with AppDaemon APIs.
* Store authoritative automation state in Home Assistant.

The project should also avoid becoming a second configuration language for Home Assistant. The value is writing automations as Rust code, not inventing another YAML/TOML automation DSL.

## Suggested implementation shape

### Home Assistant client dependency

The MVP should not depend on [`hass-rs`](https://github.com/danrusei/hass-rs).
It provides useful basic Home Assistant WebSocket operations, including
authentication, service calls, state queries, and event subscriptions, but its
connection model does not match the runtime requirements of this project.

In particular, `hauto` needs:

```text
automatic reconnect and reliable connection-loss notification
timeouts and completion of pending requests on every failure path
concurrent use by multiple automation tasks
event fan-out that cannot block the WebSocket reader on a slow consumer
raw commands and service-call response payloads
REST state publishing
```

At the time of this proposal, `hass-rs` lists automatic reconnection as
unfinished, exposes most client operations through a mutable client reference,
discards successful service-call response data, and does not provide the REST
state publishing needed here. Its event delivery is also coupled to bounded
subscriber channels inside the socket reader. Wrapping it would therefore
require `hauto` to implement most of its critical connection lifecycle around
an API whose failure and backpressure behavior it does not control.

The raw client should instead be implemented directly on
`tokio-tungstenite`, an HTTP client, `serde`, and `serde_json`. WebSocket
transport should handle commands and events; HTTP transport should handle REST
operations such as publishing states. This is deliberately limited to the
protocol and lifecycle behavior needed by `hauto`; it is not an attempt to
model the complete Home Assistant API.

This decision can be revisited if `hass-rs` gains a connection lifecycle,
concurrency model, and raw request interface that satisfy these requirements.

### Runtime

Use Tokio as the async runtime.

Core runtime responsibilities:

```text
connect to Home Assistant WebSocket API
authenticate
subscribe to state_changed events
fan out events to automation tasks
provide service-call API
publish states through the REST API
manage timers
handle reconnects
surface automation failures
```

Suggested core types:

```rust
struct App;
struct Context;
struct HomeAssistantClient;
struct EntityId;
struct EntityState;
struct StateWrite;
enum SetStateResult;
enum DeleteStateResult;
struct StateChangedEvent;
struct StateWait;
struct TimedStateWait;
struct StateExpectation;
struct TaskHandle<T>;
struct TimerHandle<T>;
enum EventStreamError;
enum WaitResult;
enum HoldResult<T>;
enum TimeoutResult<T>;
trait Automation;
```

Possible API:

```rust
#[async_trait]
trait Automation {
    async fn run(self, ctx: Context) -> Result<()>;
}
```

`App` should expose separate registration methods for structured automations
and async functions so their contracts are not overloaded:

```rust
fn automation<A, F>(self, name: impl Into<String>, factory: F) -> Self
where
    A: Automation,
    F: Fn() -> A + Send + Sync + 'static;

fn automation_fn<F, Fut>(self, name: impl Into<String>, run: F) -> Self
where
    F: Fn(Context) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static;
```

The runtime invokes the factory or function once per connection generation.
It does not invoke either again after normal completion or failure within that
generation.

Registration:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::var("HOME_ASSISTANT_URL")?;
    let token = std::env::var("HOME_ASSISTANT_TOKEN")?;
    let light = Light::new("light.office")?;
    let occupancy = BinarySensor::new("binary_sensor.office_occupancy")?;

    App::new(url, token)
        .automation("office occupancy light", move || OccupancyLight {
            light: light.clone(),
            occupancy: occupancy.clone(),
            dim_after: Duration::from_secs(30),
            off_after: Duration::from_secs(30),
        })
        .run()
        .await
}
```

### Raw client

Each `Context` should expose a cloneable, generation-scoped raw handle through
`ctx.home_assistant()`. A cloned handle remains bound to that generation and
must fail after it ends rather than silently attaching to a new connection.
The raw client should expose:

```rust
async fn call_service_raw(
    &self,
    domain: &str,
    service: &str,
    data: serde_json::Value,
) -> Result<serde_json::Value>;

async fn command_raw(
    &self,
    command: serde_json::Value,
) -> Result<serde_json::Value>;

async fn set_state_raw(
    &self,
    entity_id: &EntityId,
    state: StateWrite,
) -> Result<SetStateResult>;

async fn delete_state_raw(
    &self,
    entity_id: &EntityId,
) -> Result<DeleteStateResult>;

async fn get_state_raw(
    &self,
    entity_id: &EntityId,
) -> Result<EntityState>;

async fn subscribe_state_changes(
    &self,
) -> Result<impl Stream<Item = Result<StateChangedEvent, EventStreamError>>>;

async fn subscribe_events_raw(
    &self,
    event_type: Option<&str>,
) -> Result<impl Stream<Item = Result<serde_json::Value, EventStreamError>>>;
```

`command_raw` should accept a JSON object containing a Home Assistant command
`type` and its command-specific fields. The client owns request IDs: it should
reject a caller-supplied `id`, insert the next ID, correlate the response, and
return the raw result. Streaming commands should use a dedicated subscription
method rather than `command_raw`. Raw commands should never be retried
automatically; if a command may have been written before connection loss, it
should also fail with `OutcomeUnknown`.

`get_state_raw` should read the current generation's cache rather than issuing
another Home Assistant request. `set_state_raw` and `delete_state_raw` should
use the REST behavior defined under state publishing. Raw event subscriptions
are scoped to one connection generation and terminate on connection loss like
typed streams.

### Typed layer

The typed layer should be small and practical.

Initial typed domains:

```text
Light
Switch
BinarySensor
Sensor<String>
Sensor<f64>
```

Initial typed services:

```text
light.turn_on
light.turn_off
switch.turn_on
switch.turn_off
homeassistant.turn_on
homeassistant.turn_off
```

The guiding principle should remain: typed where it improves personal
automation code, dynamic where Home Assistant itself is dynamic.

## Future direction: WebAssembly automations

WebAssembly automation loading is explicitly outside the MVP, but it is a
plausible future frontend for the same runtime. It could allow automations to
be loaded or updated without rebuilding the daemon, isolate traps and resource
usage, and eventually support guest languages other than Rust.

The intended architecture would keep native and WebAssembly automation APIs
over the same internal runtime operations:

```text
native Rust API ─────┐
                     ├── runtime operations ── Home Assistant
WebAssembly guest API┘
```

[`Wasmtime`'s async Component Model](https://docs.wasmtime.dev/api/wasmtime/component/index.html)
supports async functions, futures, streams, and concurrent guest tasks. A
WebAssembly automation could therefore retain the long-running async
state-machine style rather than being reduced to synchronous event callbacks.

Native Rust types are not themselves a stable component boundary. `Context`,
borrowed builders such as `StateWait<'a>`, generic Rust enums, and traits would
need a versioned WIT interface using owned component-model types. A guest-side
Rust crate could recreate the native ergonomics over generated async bindings:

```text
guest Context / StateWait / StateExpectation
                    ↓
versioned async WIT interface
                    ↓
host runtime operations
```

The WIT interface should expose capabilities rather than the Home Assistant
token or unrestricted host access. Likely capabilities include:

```text
read cached entity state
receive state-change streams
sleep and observe cancellation
call allowed services
publish and delete allowed ephemeral states
```

The host should remain responsible for connection generations, state caching,
timers, service delivery, and cancellation. A component instance should belong
to one automation generation. Connection loss, explicit reload, or shutdown
should cancel its guest tasks and discard the instance; a fresh instance
should be created when the automation restarts.

Filesystem, network, entity, and service access should be denied unless
explicitly granted. Wasmtime resource controls should bound guest memory and
CPU execution so a trapped or non-cooperative guest cannot compromise other
automations or block the daemon.

This direction depends on a stable versioned WIT contract and sufficiently
mature component-async guest tooling. It should be implemented as an adapter
over the native runtime primitives rather than shaping or delaying the MVP API.
