# Proposal: Entity Discovery and Area-based Example Configuration

## Motivation

`hauto` currently requires callers to know every Home Assistant entity ID
before `App` connects:

```rust
let temperature =
    Sensor::<SensorValue<f64>>::new("sensor.main_bathroom_temperature")?;
let humidity =
    Sensor::<SensorValue<f64>>::new("sensor.main_bathroom_humidity")?;
let occupancy =
    BinarySensor::new("binary_sensor.main_bathroom_occupancy")?;
```

This is explicit and reliable, but it makes automations that operate on rooms
or areas unnecessarily installation-specific. Home Assistant already knows
which entities belong to an area and classifies common sensors with
`device_class` attributes such as `temperature`, `humidity`, `occupancy`, and
`motion`.

The concrete first caller is `examples/bathroom_exhaust_fan`. Its normal
configuration currently needs entity IDs for:

* two bathroom temperature sensors;
* two bathroom humidity sensors;
* two bathroom occupancy sensors;
* an ambient-room temperature sensor;
* an ambient-room humidity sensor;
* two derived humidity status entities; and
* the shared exhaust fan switch.

For a well-organized Home Assistant installation, the user should instead be
able to provide the three area names and the fan switch's display name. The
example can discover the input entities from their domains and device classes,
while retaining explicit entity-ID overrides for installations with ambiguous
or incomplete metadata.

## Problem statement

The current hauto API cannot implement reliable area-based discovery:

1. `GenerationState` caches all current states, but public APIs only retrieve
   state by a known `EntityId`.
2. `EntityState` includes useful attributes such as `device_class` and
   `friendly_name`, but it does not include reliable area membership.
3. Area membership may be assigned directly to an entity or inherited from its
   device. Reimplementing this relationship from state attributes would be
   incorrect.
4. `HomeAssistantClient::command_raw` can call the relevant WebSocket commands,
   but callers would have to parse unstable JSON shapes and duplicate query,
   matching, and ambiguity behavior.
5. The bathroom example constructs its typed handles before `App::run`, while
   discovery requires an authenticated WebSocket `Context`.

Selecting the first matching entity is not an acceptable fallback. A bathroom
may contain multiple temperature or motion sensors, and silently selecting one
would make the automation dependent on registry ordering.

## Proposal

Add a read-only, per-connection `EntityCatalog` and a small query API. The
catalog should expose Home Assistant areas and enabled registry entities,
resolve area membership through Home Assistant, join registry entries with the
current state cache, and convert an exact query result into existing typed
entity handles.

The query API should remain general. The bathroom-specific policy about which
device classes represent its inputs belongs in the example.

### Public data types

Add `AreaId`, `AreaInfo`, `DiscoveredEntity`, `EntityCatalog`, `EntitySet`, and
`EntityQuery` in a new `src/discovery.rs` module and re-export the user-facing
types from `src/lib.rs`.

The public types should expose metadata needed to understand and refine a
query without exposing Home Assistant's abbreviated registry response:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AreaId(/* private String */);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaInfo {
    // private fields
}

impl AreaInfo {
    pub fn id(&self) -> &AreaId;
    pub fn name(&self) -> &str;
}

#[derive(Clone, Debug)]
pub struct DiscoveredEntity {
    // private fields
}

impl DiscoveredEntity {
    pub fn entity_id(&self) -> &EntityId;
    pub fn name(&self) -> Option<&str>;
    pub fn device_class(&self) -> Option<&str>;

    pub fn binary_sensor(&self) -> Result<BinarySensor>;
    pub fn sensor<T>(&self) -> Result<Sensor<T>>;
    pub fn switch(&self) -> Result<Switch>;
}
```

Typed conversion should call the existing constructors so domain validation
has exactly the same behavior as manually creating an entity handle. The first
version does not need a general `EntityHandle` trait.

`DiscoveredEntity::name()` should be the effective display name used for
matching. Prefer the current state's `friendly_name`, then the entity
registry's user or original name. Entity IDs remain available for diagnostics
and explicit selection.

### Loading the catalog

Add an async method to `Context`:

```rust
impl Context {
    pub async fn entity_catalog(&self) -> Result<EntityCatalog>;
}
```

The catalog should use typed internal wrappers around these Home Assistant
WebSocket commands:

```text
config/area_registry/list
config/entity_registry/list_for_display
extract_from_target
```

The first two commands provide area names and enabled entity metadata.
`extract_from_target` should resolve the entities belonging to a requested
area:

```json
{
  "type": "extract_from_target",
  "target": {
    "area_id": ["main_bathroom"]
  }
}
```

Using `extract_from_target` is preferable to reproducing Home Assistant's
entity-area and device-area inheritance rules in hauto.

Catalog metadata should be cached in `GenerationState`, shared by every clone
of a `Context`. Area target results should also be memoized by `AreaId`. A
`tokio::sync::OnceCell` or equivalent per-generation structure can coalesce
concurrent catalog initialization by the bathroom example's three
automations.

The catalog is a connection-generation snapshot:

* a reconnect creates a new generation and reloads discovery metadata;
* state values continue to update through the existing state cache;
* registry edits during a healthy connection do not update the catalog in the
  first version.

Users who change areas or names while hauto is running may restart hauto to
reload discovery metadata. Live registry subscriptions can be added after a
real use case requires them.

Malformed registry responses should be reported as a protocol/connection
error, not interpreted as an empty catalog.

### Area lookup

Expose exact area lookup:

```rust
let catalog = ctx.entity_catalog().await?;
let bathroom = catalog.area_named("Main Bathroom")?;
```

Area names should be trimmed and compared case-insensitively, but should not
use substring or fuzzy matching. A missing area returns a dedicated error that
includes the requested name. If Home Assistant ever returns multiple matching
names, lookup should report ambiguity rather than selecting one.

### Entity sets and queries

Expose all enabled catalog entities and area-scoped entity sets:

```rust
let all_entities = catalog.entities();
let bathroom_entities = catalog.entities_in(&bathroom).await?;
```

`entities_in` is async because the first lookup may issue
`extract_from_target`. Later lookups for the same area in the generation use
the memoized result.

An `EntitySet` should create a query builder:

```rust
let temperature = bathroom_entities
    .query()
    .domain("sensor")
    .device_class("temperature")
    .exactly_one()?
    .sensor::<SensorValue<f64>>()?;
```

Initial filters should be deliberately small:

```rust
impl EntityQuery {
    pub fn domain(self, domain: impl Into<String>) -> Self;
    pub fn device_class(self, device_class: impl Into<String>) -> Self;
    pub fn device_class_in<I, S>(self, device_classes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>;
    pub fn named(self, name: impl Into<String>) -> Self;

    pub fn collect(self) -> Vec<DiscoveredEntity>;
    pub fn exactly_one(self) -> Result<DiscoveredEntity>;
}
```

Domain and device-class matching should be exact and case-sensitive because
these are Home Assistant identifiers. Name matching should use the same
trimmed, case-insensitive exact comparison as area names. Fuzzy matching,
ranking, and implicit preference by registry order are out of scope.

`exactly_one()` should have three observable outcomes:

* one match returns that entity;
* no matches returns an entity-query-not-found error containing a description
  of the filters;
* multiple matches returns an entity-query-ambiguous error containing the
  filters and every candidate's entity ID, name, and device class.

An entity requiring a `device_class` filter must have a current cached state
with that attribute. Missing, `unknown`, and `unavailable` state values do not
exclude an otherwise matching entity; availability is a runtime concern
handled by `SensorValue` and `BinaryState`.

The entity registry display command excludes disabled entries. Hidden entries
should remain queryable because hiding affects presentation, not whether an
automation may use the entity.

### Error surface

Add explicit error variants rather than reducing discovery failures to
`EntityNotFound`:

```rust
Error::AreaNotFound { name: String }
Error::AreaAmbiguous {
    name: String,
    candidates: Vec<AreaId>,
}
Error::EntityQueryNotFound { query: String }
Error::EntityQueryAmbiguous {
    query: String,
    candidates: Vec<EntityId>,
}
```

The `Display` output for an ambiguous entity query should include enough
candidate metadata to resolve the configuration problem without enabling
debug logging. The exact internal representation may retain richer candidate
details than the public variant if needed.

### Bathroom exhaust example

Refactor `examples/bathroom_exhaust_fan/main.rs` around this default
configuration:

```sh
export HAUTO_BATHROOM_1_AREA='Main Bathroom'
export HAUTO_BATHROOM_2_AREA='Ensuite'
export HAUTO_AMBIENT_AREA='Hall'
export HAUTO_EXHAUST_FAN_NAME='Bathroom Exhaust Fan'
```

The Home Assistant URL, token, and existing optional quiet-hour variables
remain unchanged.

For each bathroom area, resolve:

```text
temperature: sensor + device_class=temperature
humidity:    sensor + device_class=humidity
occupancy:   binary_sensor + device_class in {occupancy, motion}
```

Do not include `presence` in the default occupancy query. Home Assistant uses
that class for home/away-style presence as well as room presence, so treating
it as bathroom occupancy without explicit configuration is unsafe.

Resolve the ambient temperature and humidity from the ambient area with the
same sensor-domain and device-class filters. Resolve the fan globally as:

```text
domain=switch + exact effective display name
```

Every query must use `exactly_one()`. If an area contains multiple candidates,
the example should fail during generation startup with the candidate list
rather than choose one.

### Explicit override behavior

Keep the current entity-ID environment variables as optional overrides:

```text
HAUTO_BATHROOM_1_TEMP
HAUTO_BATHROOM_1_HUMIDITY
HAUTO_BATHROOM_1_OCCUPANCY
HAUTO_BATHROOM_1_HUMIDITY_STATUS
HAUTO_BATHROOM_2_TEMP
HAUTO_BATHROOM_2_HUMIDITY
HAUTO_BATHROOM_2_OCCUPANCY
HAUTO_BATHROOM_2_HUMIDITY_STATUS
HAUTO_AMBIENT_TEMP
HAUTO_AMBIENT_HUMIDITY
HAUTO_EXHAUST_FAN
```

For each role:

1. if its explicit entity-ID variable is present, construct the typed handle
   from it and do not query for that role;
2. otherwise, use area/name discovery;
3. propagate domain-validation, no-match, and ambiguity errors.

This makes the area configuration the concise normal path without removing a
deterministic escape hatch for rooms containing multiple suitable entities.
An area-name variable is required only when at least one role in that area
needs discovery or its derived status ID must be generated. Similarly,
`HAUTO_EXHAUST_FAN_NAME` is required only when `HAUTO_EXHAUST_FAN` is absent.
Supplying every existing entity-ID variable therefore remains a valid
configuration.

### Generation-time example setup

Discovery cannot run before `App::run` because it requires an authenticated
WebSocket connection. Keep `App` unchanged and resolve each automation's
configuration at the start of its `automation_fn` future:

```rust
App::new(url, token)
    .automation_fn("bathroom 1 humidity status", move |ctx| {
        let spec = bathroom_1.clone();
        async move {
            let config = resolve_humidity_config(&ctx, spec).await?;
            HumidityStatus::new(config).run(ctx).await
        }
    })
```

The fan controller follows the same shape. The shared generation catalog
prevents the three automations from independently loading registry metadata,
and the App's existing reconnect behavior naturally reruns discovery against
the new generation.

Put the example-specific resolution helpers in
`examples/bathroom_exhaust_fan/discovery.rs` rather than expanding `main.rs`.
`humidity_status.rs` and `fan_control.rs` should continue receiving concrete
typed handles and should not know how those handles were selected.

### Derived humidity status entity IDs

When an explicit `HAUTO_BATHROOM_N_HUMIDITY_STATUS` is absent, derive a stable
raw state ID from the resolved Home Assistant area ID:

```text
sensor.hauto_<area_id>_excess_humidity
```

Both the humidity publisher and fan controller can derive the same ID without
querying for it. The `hauto_` prefix reduces the risk of colliding with an
integration-owned entity.

These sensors are still created with `set_state_raw`. They do not have entity
registry entries, do not survive a Home Assistant restart independently of
hauto, and cannot be assigned to Home Assistant areas. Adding an `area_id`
state attribute would not create a real area assignment. Supporting
area-assigned generated entities requires a future entity-registration
mechanism such as a Home Assistant integration or MQTT discovery and is not
part of this proposal.

## Non-goals

* **Automatically choosing among multiple matches:** Ambiguity is an error.
  There is no scoring by entity ID, platform, update time, or registry order.
* **A bathroom-specific core API:** Hauto provides catalog and query
  primitives; the example defines temperature, humidity, and occupancy roles.
* **Fuzzy names:** Area and entity names use exact normalized matching only.
* **Live registry synchronization:** The first version refreshes discovery
  metadata on connection generation startup, not on registry update events.
* **Registry mutation:** This proposal does not rename entities, move entities
  between areas, or create entity registry entries.
* **Area assignment for REST-created states:** Derived status sensors remain
  transient state-machine entities.
* **A comprehensive Home Assistant metadata model:** Only metadata required by
  the first query API is represented. Floors, labels, integrations, platforms,
  and device identifiers can be added for concrete callers later.
* **Removing explicit entity IDs:** Existing environment variables remain
  usable as overrides.
* **Changing `App` initialization hooks:** Automation functions resolve their
  own concrete configuration from the connected `Context`.

## Suggested implementation shape

1. Add typed internal response structures and WebSocket client methods for
   area listing, entity listing, and target extraction.
2. Add per-generation catalog storage and memoized area membership to
   `GenerationState` in `src/client.rs`.
3. Implement `AreaId`, `AreaInfo`, `DiscoveredEntity`, `EntityCatalog`,
   `EntitySet`, and `EntityQuery` in `src/discovery.rs`.
4. Add `Context::entity_catalog()` and re-export the public discovery types.
5. Add discovery error variants with useful no-match and ambiguity messages.
6. Add `examples/bathroom_exhaust_fan/discovery.rs` with optional override
   handling and bathroom-specific queries.
7. Refactor `examples/bathroom_exhaust_fan/main.rs` to read area/name specs and
   resolve concrete configs inside automation functions.
8. Update `examples/bathroom_exhaust_fan/README.md` with the concise default
   configuration, override table, ambiguity behavior, and derived-status
   limitations.

## Verification

Verification should include:

```sh
cargo fmt --check
CARGO_TARGET_DIR=/tmp/hauto-target cargo test
CARGO_TARGET_DIR=/tmp/hauto-target cargo test --example bathroom_exhaust_fan
CARGO_TARGET_DIR=/tmp/hauto-target cargo check --examples
CARGO_TARGET_DIR=/tmp/hauto-target cargo clippy --all-targets -- -D warnings
```

Specific tests should prove:

* area and entity registry responses decode into public catalog metadata;
* `entity_catalog()` is loaded once when multiple cloned contexts request it
  concurrently;
* area names use trimmed, case-insensitive exact matching;
* `entities_in()` sends `extract_from_target` with the resolved area ID;
* repeated queries for one area reuse its memoized target result;
* domain, device-class, device-class-set, and exact-name filters compose;
* unavailable and unknown states do not prevent metadata matches;
* a device-class query does not match an entity lacking that attribute;
* `exactly_one()` reports no matches separately from multiple matches;
* ambiguity errors list every candidate entity ID;
* typed conversion rejects a result from the wrong domain;
* cancellation or connection loss during catalog loading returns the existing
  generation cancellation/connection error;
* a new connection generation does not reuse the prior generation's catalog;
* explicit example overrides bypass discovery for their role;
* bathroom discovery rejects multiple temperature, humidity, or occupancy
  candidates;
* the example accepts one `occupancy` or one `motion` binary sensor but rejects
  a room containing both;
* generated humidity status IDs are stable and derived from area IDs; and
* the bathroom example compiles with discovery performed inside its
  automation functions.

## Success criteria

* A connected `Context` can load a shared, read-only entity catalog.
* Callers can query enabled entities by area, domain, device class, and exact
  display name.
* Queries never silently select one entity from multiple matches.
* Query results convert into the existing typed entity handles.
* The bathroom example's normal configuration requires three area names and
  one fan switch name instead of all input entity IDs.
* Existing entity-ID variables remain supported as deterministic overrides.
* Reconnection reruns discovery from a fresh generation.
* The documentation clearly states that raw derived sensors cannot receive
  Home Assistant area assignments.
