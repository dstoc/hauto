# Proposal: Organize and document the public API

## Motivation

`hauto` has grown from a small runtime API into a framework with typed entity
handles, waits and expectations, state streams, raw Home Assistant operations,
entity discovery, timers, and state publishing. All of these APIs are currently
re-exported from `src/lib.rs`:

```rust
use hauto::{
    App, AreaInfo, BinarySensor, Context, DiscoveredEntity, EntityCatalog,
    EntityId, EntityState, GlobalStateWait, HoldResult, HomeAssistantClient,
    Light, LightTurnOn, RawEventStream, Sensor, SensorValue, StateCache,
    StateExpectation, StateWrite, TaskHandle, TimerHandle, WaitResult,
};
```

The flat namespace keeps short examples convenient, but it no longer
communicates which types are normal entrypoints, returned implementation
types, discovery APIs, or low-level escape hatches. Generated rustdoc presents
one largely alphabetical list instead of guiding users through the framework's
concepts.

The crate-level documentation and `README.md` explain the main runtime model,
but most public types, variants, fields, and methods have no rustdoc. Important
behavior is consequently only discoverable by reading the implementation or
the proposal history. Examples include:

* whether an entity constructor checks Home Assistant for existence;
* whether a wait accepts an already-matching initial state;
* what reconnect cancellation does to a held condition;
* how `unknown`, `unavailable`, and a missing entity differ;
* when a state stream can end or report lag; and
* which client methods are raw protocol escape hatches.

The public API is still explicitly early. This is the appropriate point to
establish its navigation and documentation conventions before more entity
types and runtime entrypoints are added.

## Problem statement

The current API has two related problems.

First, `src/lib.rs` erases the useful conceptual boundaries already present in
the implementation. `EntityCatalog`, `TimedGlobalStateWait`,
`HomeAssistantClient`, and `App` all appear to have equal prominence even
though users normally construct only `App` and typed entity handles. Some
public types exist primarily because they occur in method return types and do
not normally need to be imported.

Second, the crate does not enforce public documentation. `cargo doc` succeeds
because missing rustdoc is allowed, not because the public contracts are
documented. Adding more APIs under the current policy will make filling the
documentation gap progressively harder.

Splitting the implementation into more crates would not solve either problem.
These concepts form one framework API and share runtime internals. The missing
boundary is a Rust module and documentation boundary, not a package boundary.

## Proposal

Expose a small set of public modules organized around user-facing concepts,
retain a curated set of common root re-exports, and document every public API.

The intended shape is:

```text
hauto
├── App, Automation, Context
├── BinarySensor, Light, Sensor, Switch
├── BinaryState, SensorValue
├── LightTurnOn, LightTurnOff
├── HoldResult, WaitResult, TimeoutResult
├── Error, Result
│
├── entity
├── discovery
├── wait
├── state
├── runtime
├── service
└── client
```

This follows the common Rust library pattern of keeping the primary path
concise while making the complete API browsable by topic. It does not add a
`prelude`; hauto does not currently require a collection of extension traits,
so a prelude would add another import convention without solving a real
problem.

### Root namespace

The root should contain types that are routinely named by normal automation
code:

```rust
pub use runtime::{App, Automation, Context};
pub use entity::{BinarySensor, EntityId, Light, Sensor, Switch};
pub use entity::{BinaryState, SensorValue};
pub use service::{LightTurnOff, LightTurnOn};
pub use wait::{HoldResult, TimeoutResult, WaitResult};
pub use error::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

`EntityId` remains at the root because raw service calls and state publishing
both use it directly. The three result enums remain at the root because callers
must commonly pattern-match on them. Concrete wait builders, stream types,
catalog types, and raw state representations should use their module paths.

Common code remains compact:

```rust
use hauto::{App, BinarySensor, HoldResult, Light, LightTurnOff, LightTurnOn};
```

Advanced code becomes self-describing:

```rust
use hauto::discovery::{EntityCatalog, EntitySet};
use hauto::runtime::TimerHandle;
use hauto::state::{EntityState, StateWrite};
```

The crate is still at `0.1.0` and its README calls the API early. Existing
advanced root re-exports should be removed and all repository examples should
be updated in the same change. Keeping every old root export would preserve the
flat rustdoc surface and undermine the purpose of the organization.

This proposal does not rename existing types. Names such as `LightTurnOn` may
be reconsidered separately, but namespace organization should not be mixed
with a service-options naming redesign.

### `entity`

The `entity` module owns entity identity, typed handles, and the typed values
decoded by those handles:

```rust
hauto::entity::{
    EntityId,
    BinarySensor,
    Light,
    Sensor,
    Switch,
    BinaryState,
    SensorValue,
}
```

`BinaryState` and `SensorValue` conceptually belong beside the typed handles
even though their implementation currently lives in `src/state.rs`.
Re-exporting them through `entity` does not require moving their internal
definitions.

The module documentation should explain:

* construction validates entity-ID syntax and domain, not current existence;
* `read` uses the generation's cached state;
* `get` performs a current Home Assistant read;
* `next_change` waits for a later event;
* strict and availability-aware sensor decoding; and
* reconnect cancellation behavior.

### `discovery`

The `discovery` module contains:

```rust
hauto::discovery::{
    AreaId,
    AreaInfo,
    DiscoveredEntity,
    EntityCatalog,
    EntityQuery,
    EntitySet,
}
```

Its module documentation should establish that a catalog is cached per
connection generation, area and name matching semantics, exclusion of disabled
registry entries, and the error behavior for zero or ambiguous matches.

These types should not also be re-exported at the crate root. Most users obtain
an `EntityCatalog` through `Context::entity_catalog` and can use inference
without importing its concrete type.

### `wait`

The `wait` module contains the complete builder and result API:

```rust
hauto::wait::{
    GlobalStateWait,
    HoldResult,
    StateExpectation,
    StateWait,
    TimedGlobalStateWait,
    TimedStateWait,
    TimeoutResult,
    WaitResult,
}
```

The result types remain root re-exports for matching convenience. Builder types
need public names because they occur in method signatures, but normal callers
do not construct them directly.

The module documentation should define the distinction between:

* a wait, which waits until a condition becomes true;
* an expectation, which checks the condition immediately;
* `for_at_least`, which requires continuous satisfaction;
* `within`, which bounds how long a wait may take; and
* `require_transition`, which rejects an already-satisfied initial state until
  the condition first becomes false and then true.

It should also state that connection loss cancels the current automation
generation. A held duration is not resumed across reconnection because `App`
restarts the automation with a new `Context`.

### `state`

The `state` module contains cached, event, and REST state representations:

```rust
hauto::state::{
    Availability,
    DeleteStateResult,
    EntityState,
    SetStateResult,
    StateCache,
    StateChangedEvent,
    StateWrite,
}
```

`BinaryState` and `SensorValue` may also remain visible from this module if
their definitions stay there, but `entity` is their documented user-facing
path. Avoid duplicating them in examples.

The module documentation should distinguish:

* a missing entity from an `unknown` or `unavailable` entity;
* the immutable `StateCache` view from a REST `get_state_raw` request;
* state-change event deletion, represented by a missing `new_state`; and
* ephemeral REST state-machine entries from entity-registry entries.

Every public field on `EntityState`, `StateWrite`, and `StateChangedEvent`
should document its Home Assistant meaning. `Availability` is currently
exported but unused by the rest of the public API; implementation should either
give it a documented role or make it private rather than retaining an
unconnected public abstraction.

### `runtime`

The `runtime` module contains:

```rust
hauto::runtime::{
    App,
    Automation,
    BoxFuture,
    Context,
    TaskHandle,
    TimerHandle,
}
```

`App`, `Automation`, and `Context` remain root re-exports. Handle types and
`BoxFuture` are advanced runtime types and should be imported through the
module.

The module documentation should describe the connection-generation lifecycle:
initial state loading, event subscription, automation startup, cancellation,
and restart. Individual APIs should state whether spawned work is
cancellation-aware, whether dropping or cancelling a handle aborts work, and
how errors from automation tasks affect `App::run`.

This organization intentionally leaves room for another entrypoint, such as a
future `hauto-bevy` crate, without factoring Home Assistant transport out of
hauto or making `App` the namespace owner of all runtime primitives.

### `service`

The `service` module initially contains:

```rust
hauto::service::{LightTurnOff, LightTurnOn}
```

Both types remain root re-exports because typed light operations are common.
The module provides a natural location for future typed service option types
without requiring every new option type to be introduced first at the root.
This proposal does not require adding wrappers for more Home Assistant
domains.

Rustdoc should document validation, mutually exclusive brightness fields,
units and ranges, omitted-value behavior, and the shape of the service call
made by each option type.

### `client`

The `client` module contains the lower-level Home Assistant client and streams:

```rust
hauto::client::{
    EventStreamError,
    HomeAssistantClient,
    RawEventStream,
    StateChangeStream,
}
```

These types should not be root re-exports. Normal automations obtain the client
through `Context::home_assistant`, while explicit imports indicate code that
depends on a lower-level API.

The module documentation should distinguish typed helpers from methods ending
in `_raw`. Raw command and event methods should document the expected Home
Assistant protocol shape, cancellation behavior, malformed-response behavior,
and stream lag or closure semantics.

The name `client` is preferred over `raw` because the type also exposes useful
typed operations such as state subscription and switch-like `turn_on` and
`turn_off`. Method names continue to identify the raw escape hatches.

### Documentation requirements

Add crate-level documentation lints:

```rust
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
```

The implementation may introduce `missing_docs` as a warning while work is in
progress, but the completed change should deny it. The lint should cover:

* every public module;
* every public type and trait;
* every public enum variant;
* every public struct field;
* every public method and associated function; and
* the `Result` and `BoxFuture` aliases.

Documentation should focus on contracts rather than restating signatures.
Each fallible operation should describe its important error cases. Each async
operation should describe cancellation and reconnect behavior when those are
observable. Builders should include a short example on their primary entry
method rather than repeating the same example on every concrete return type.

Macro-generated entity methods in `src/entity.rs` are part of this requirement.
Their macro definitions should emit rustdoc, or accept documentation text as
macro input, so generated APIs do not become an undocumented exception.

The existing crate-level guide in `src/lib.rs` should remain the primary
rustdoc landing page. It should add a short API-map section linking to the
public modules. `README.md` should remain the user-facing project overview and
should use the preferred import paths, but the two files do not need to contain
identical prose.

## Suggested implementation shape

1. Define the public module map in `src/lib.rs`, using the existing source
   modules where they already match the intended public concept.
2. Add small facade modules for concepts such as `runtime` and `service` where
   implementation files are currently split differently.
3. Reduce root re-exports to the curated common set and update all examples,
   doctests, proposal snippets that describe current API, and internal imports.
4. Add module-level documentation and rustdoc for every existing public item.
5. Audit currently exported but unused abstractions such as `Availability`;
   either connect and document them or make them private.
6. Enable `missing_docs` and broken-link lints once the public surface is
   complete.
7. Generate and inspect rustdoc to confirm that the landing page and module
   navigation reflect the intended hierarchy.

The source files do not need to be split solely to mirror the public
namespace. Re-exports and small facade modules are sufficient where an
implementation type naturally participates in more than one public concept.

## Non-goals

* **Splitting hauto into multiple crates:** The current problem is API
  navigation and documentation, not compilation boundaries.
* **Adding a prelude:** There is no trait-import burden that justifies one.
* **Renaming existing public types:** Naming changes can be proposed
  separately after the module organization is visible.
* **Adding new entity or service wrappers:** This proposal organizes and
  documents the API that exists.
* **Hiding all concrete builder types:** Public return types must remain
  nameable and documentable even when callers normally rely on inference.
* **Generating an exhaustive Home Assistant protocol reference:** Raw methods
  should document their contract and link to relevant Home Assistant concepts,
  not duplicate the upstream protocol documentation.
* **Treating examples as a substitute for rustdoc:** Examples demonstrate
  composition; public items still need local contract documentation.

## Verification

Run the normal checks:

```sh
cargo fmt --check
CARGO_TARGET_DIR=/tmp/hauto-target cargo test
CARGO_TARGET_DIR=/tmp/hauto-target cargo check --examples
CARGO_TARGET_DIR=/tmp/hauto-target cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" CARGO_TARGET_DIR=/tmp/hauto-target \
    cargo doc --no-deps
```

Also inspect the generated crate landing page and verify:

* common automation code is represented by the root namespace;
* advanced APIs are navigable through conceptual modules;
* every public module has an overview;
* no public item is undocumented;
* intra-doc links resolve; and
* examples and doctests use the preferred paths.

Add a compile-only integration test that imports the intended root API and one
that imports each advanced module. This catches accidental re-flattening or
loss of a documented module path independently of implementation tests.

## Success criteria

* The crate root exposes only the documented common API set.
* Entity, discovery, wait, state, runtime, service, and client APIs are
  browsable through public modules.
* Every public item has contract-focused rustdoc.
* Missing documentation and broken rustdoc links fail CI.
* Existing examples compile using the preferred paths.
* Runtime behavior and Home Assistant protocol semantics are unchanged.
