//! Generation-scoped discovery of Home Assistant areas and entities.
//!
//! [`EntityCatalog`](crate::discovery::EntityCatalog) loads the area registry
//! and the entity-registry display
//! catalog once per connection generation; clones and repeated requests in that
//! generation reuse the snapshot. The display catalog excludes disabled
//! registry entries but retains hidden entries. Cached state contributes
//! `friendly_name` and `device_class` metadata at snapshot creation.
//!
//! Area and entity-name lookup is trimmed, case-insensitive exact matching.
//! Domain and device-class filters are case-sensitive exact identifier matches.
//! Singular lookups report distinct zero-match and ambiguous-match errors.

use std::{collections::HashSet, fmt, sync::Arc};

use serde::Deserialize;

use crate::{
    Error, Result,
    client::{GenerationState, HomeAssistantClient},
    entity::{BinarySensor, EntityId, Sensor, Switch},
};

/// A Home Assistant area registry identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AreaId(String);

impl AreaId {
    /// Returns the identifier exactly as supplied by Home Assistant.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AreaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AreaId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// The stable ID and display name of a discovered Home Assistant area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaInfo {
    id: AreaId,
    name: String,
}

impl AreaInfo {
    /// Returns this area's registry identifier.
    pub fn id(&self) -> &AreaId {
        &self.id
    }

    /// Returns this area's Home Assistant display name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Entity identity and query metadata captured in a discovery snapshot.
///
/// The name prefers the generation cache's `friendly_name` and otherwise uses
/// the registry display name. Device class is present only when it existed in
/// cached state when the catalog was created.
#[derive(Clone, Debug)]
pub struct DiscoveredEntity {
    entity_id: EntityId,
    name: Option<String>,
    device_class: Option<String>,
}

impl DiscoveredEntity {
    /// Returns the entity's validated Home Assistant entity ID.
    pub fn entity_id(&self) -> &EntityId {
        &self.entity_id
    }

    /// Returns the snapshot display name, if Home Assistant supplied one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the cached `device_class` metadata, if available.
    pub fn device_class(&self) -> Option<&str> {
        self.device_class.as_deref()
    }

    /// Converts this result to a typed binary-sensor handle.
    ///
    /// Returns [`Error::InvalidDomain`] if the discovered entity is not in the
    /// `binary_sensor` domain. Conversion does not contact Home Assistant.
    pub fn binary_sensor(&self) -> Result<BinarySensor> {
        BinarySensor::new(self.entity_id.as_str())
    }

    /// Converts this result to a typed sensor handle with decoding policy `T`.
    ///
    /// Returns [`Error::InvalidDomain`] if the discovered entity is not in the
    /// `sensor` domain. `T` is applied only when state is later decoded.
    pub fn sensor<T>(&self) -> Result<Sensor<T>> {
        Sensor::new(self.entity_id.as_str())
    }

    /// Converts this result to a typed switch handle.
    ///
    /// Returns [`Error::InvalidDomain`] if the discovered entity is not in the
    /// `switch` domain. Conversion does not contact Home Assistant.
    pub fn switch(&self) -> Result<Switch> {
        Switch::new(self.entity_id.as_str())
    }
}

/// An immutable entity and area catalog cached for one connection generation.
///
/// Catalog contents and state-derived metadata do not refresh within a
/// generation. Obtain a new catalog from the restarted [`Context`](crate::Context)
/// after reconnect.
#[derive(Clone, Debug)]
pub struct EntityCatalog {
    client: HomeAssistantClient,
    snapshot: Arc<CatalogSnapshot>,
}

impl EntityCatalog {
    pub(crate) async fn load(client: HomeAssistantClient) -> Result<Self> {
        let snapshot = client.discovery_catalog().await?;
        Ok(Self { client, snapshot })
    }

    /// Finds exactly one area by trimmed, case-insensitive exact display name.
    ///
    /// Returns [`Error::AreaNotFound`] for no match and
    /// [`Error::AreaAmbiguous`] with every matching area ID for multiple
    /// matches.
    pub fn area_named(&self, name: impl AsRef<str>) -> Result<AreaInfo> {
        let requested = name.as_ref();
        let normalized = normalize_name(requested);
        let matches = self
            .snapshot
            .areas
            .iter()
            .filter(|area| normalize_name(&area.name) == normalized)
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [area] => Ok((*area).clone()),
            [] => Err(Error::AreaNotFound {
                name: requested.to_string(),
            }),
            candidates => Err(Error::AreaAmbiguous {
                name: requested.to_string(),
                candidates: candidates.iter().map(|area| area.id.clone()).collect(),
            }),
        }
    }

    /// Returns all enabled entities in this catalog snapshot.
    ///
    /// Hidden entities remain included. Use [`EntitySet::query`] to filter the
    /// returned set.
    pub fn entities(&self) -> EntitySet {
        EntitySet {
            entities: self.snapshot.entities.clone(),
        }
    }

    /// Returns catalog entities targeted by the given area.
    ///
    /// Area membership is fetched with Home Assistant's
    /// `extract_from_target` command and cached per area for this generation.
    /// Cancellation or a command/protocol failure is returned as [`Error`].
    pub async fn entities_in(&self, area: &AreaInfo) -> Result<EntitySet> {
        let membership = self.client.discovery_entities_in(&area.id).await?;
        Ok(EntitySet {
            entities: self
                .snapshot
                .entities
                .iter()
                .filter(|entity| membership.contains(&entity.entity_id))
                .cloned()
                .collect(),
        })
    }
}

fn string_attribute(state: &crate::state::EntityState, key: &str) -> Option<String> {
    state
        .attributes
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// A cloneable snapshot subset from which an [`EntityQuery`] can be built.
#[derive(Clone, Debug)]
pub struct EntitySet {
    entities: Vec<DiscoveredEntity>,
}

impl EntitySet {
    /// Starts a query containing every entity in this set.
    pub fn query(&self) -> EntityQuery {
        EntityQuery {
            entities: self.entities.clone(),
            domain: None,
            device_classes: None,
            name: None,
        }
    }
}

/// A consuming builder for exact discovery filters.
///
/// Filters combine with logical AND. Repeating a filter replaces its previous
/// value rather than adding another constraint.
#[derive(Clone, Debug)]
pub struct EntityQuery {
    entities: Vec<DiscoveredEntity>,
    domain: Option<String>,
    device_classes: Option<Vec<String>>,
    name: Option<String>,
}

impl EntityQuery {
    /// Restricts matches to an exact, case-sensitive entity-ID domain.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Restricts matches to one exact, case-sensitive device class.
    ///
    /// Entities without cached `device_class` metadata do not match.
    pub fn device_class(mut self, device_class: impl Into<String>) -> Self {
        self.device_classes = Some(vec![device_class.into()]);
        self
    }

    /// Restricts matches to any listed exact, case-sensitive device class.
    ///
    /// An empty input matches no entity; entities without device-class metadata
    /// do not match.
    pub fn device_class_in<I, S>(mut self, device_classes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.device_classes = Some(device_classes.into_iter().map(Into::into).collect());
        self
    }

    /// Restricts matches to a trimmed, case-insensitive exact display name.
    ///
    /// Entities without a catalog display name do not match.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Returns every matching entity in catalog order.
    ///
    /// An empty vector is a successful query result.
    pub fn collect(self) -> Vec<DiscoveredEntity> {
        self.matching_entities()
    }

    /// Requires exactly one matching entity.
    ///
    /// Returns [`Error::EntityQueryNotFound`] for zero matches. Multiple
    /// matches return [`Error::EntityQueryAmbiguous`] with all candidate IDs;
    /// its query description also includes candidate names and device classes.
    pub fn exactly_one(self) -> Result<DiscoveredEntity> {
        let description = self.description();
        let matches = self.matching_entities();
        match matches.as_slice() {
            [entity] => Ok(entity.clone()),
            [] => Err(Error::EntityQueryNotFound { query: description }),
            candidates => Err(Error::EntityQueryAmbiguous {
                query: format!(
                    "{description}; candidate metadata: {}",
                    candidates
                        .iter()
                        .map(|entity| format!(
                            "{} (name={}, device_class={})",
                            entity.entity_id,
                            entity.name.as_deref().unwrap_or("<none>"),
                            entity.device_class.as_deref().unwrap_or("<none>")
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                candidates: candidates
                    .iter()
                    .map(|entity| entity.entity_id.clone())
                    .collect(),
            }),
        }
    }

    fn matching_entities(&self) -> Vec<DiscoveredEntity> {
        self.entities
            .iter()
            .filter(|entity| {
                self.domain
                    .as_ref()
                    .is_none_or(|domain| entity.entity_id.domain() == domain)
                    && self.device_classes.as_ref().is_none_or(|classes| {
                        entity
                            .device_class
                            .as_ref()
                            .is_some_and(|device_class| classes.contains(device_class))
                    })
                    && self.name.as_ref().is_none_or(|name| {
                        entity.name.as_ref().is_some_and(|entity_name| {
                            normalize_name(entity_name) == normalize_name(name)
                        })
                    })
            })
            .cloned()
            .collect()
    }

    fn description(&self) -> String {
        let mut filters = Vec::new();
        if let Some(domain) = &self.domain {
            filters.push(format!("domain={domain:?}"));
        }
        if let Some(device_classes) = &self.device_classes {
            if device_classes.len() == 1 {
                filters.push(format!("device_class={:?}", device_classes[0]));
            } else {
                filters.push(format!("device_class in {device_classes:?}"));
            }
        }
        if let Some(name) = &self.name {
            filters.push(format!("name={name:?}"));
        }
        if filters.is_empty() {
            "all entities".to_string()
        } else {
            filters.join(", ")
        }
    }
}

fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogSnapshot {
    areas: Vec<AreaInfo>,
    entities: Vec<DiscoveredEntity>,
}

impl CatalogSnapshot {
    pub(crate) fn from_responses(
        areas: Vec<AreaRegistryEntry>,
        entities: EntityRegistryDisplayResponse,
        generation: &GenerationState,
    ) -> Result<Self> {
        let areas = areas
            .into_iter()
            .map(|area| {
                if area.area_id.is_empty() {
                    return Err(Error::Connection(
                        "config/area_registry/list response contained an empty area_id".to_string(),
                    ));
                }
                Ok(AreaInfo {
                    id: AreaId(area.area_id),
                    name: area.name,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            areas,
            entities: entities
                .entities
                .into_iter()
                .map(|entry| {
                    let state = generation.cached_state(&entry.entity_id);
                    let name = state
                        .as_ref()
                        .and_then(|state| string_attribute(state, "friendly_name"))
                        .or(entry.name);
                    let device_class = state
                        .as_ref()
                        .and_then(|state| string_attribute(state, "device_class"));
                    DiscoveredEntity {
                        entity_id: entry.entity_id,
                        name,
                        device_class,
                    }
                })
                .collect(),
        })
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AreaRegistryEntry {
    area_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EntityRegistryDisplayResponse {
    entities: Vec<EntityRegistryDisplayEntry>,
}

#[derive(Debug, Deserialize)]
struct EntityRegistryDisplayEntry {
    #[serde(rename = "ei")]
    entity_id: EntityId,
    #[serde(rename = "en")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExtractTargetResponse {
    pub(crate) referenced_entities: Vec<EntityId>,
}

pub(crate) type AreaMembership = HashSet<EntityId>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn entity(id: &str, name: Option<&str>, device_class: Option<&str>) -> DiscoveredEntity {
        DiscoveredEntity {
            entity_id: EntityId::new(id).unwrap(),
            name: name.map(str::to_string),
            device_class: device_class.map(str::to_string),
        }
    }

    fn set() -> EntitySet {
        EntitySet {
            entities: vec![
                entity(
                    "sensor.bathroom_temperature",
                    Some(" Bathroom Temperature "),
                    Some("temperature"),
                ),
                entity(
                    "sensor.bathroom_humidity",
                    Some("Bathroom Humidity"),
                    Some("humidity"),
                ),
                entity(
                    "binary_sensor.bathroom_motion",
                    Some("Bathroom Motion"),
                    Some("motion"),
                ),
                entity("sensor.unclassified", Some("Unclassified"), None),
            ],
        }
    }

    fn catalog(areas: Vec<AreaInfo>, entities: Vec<DiscoveredEntity>) -> EntityCatalog {
        EntityCatalog {
            client: HomeAssistantClient::default(),
            snapshot: Arc::new(CatalogSnapshot { areas, entities }),
        }
    }

    #[test]
    fn area_lookup_is_trimmed_case_insensitive_exact_and_reports_ambiguity() {
        let catalog = catalog(
            vec![
                AreaInfo {
                    id: AreaId("bathroom_one".to_string()),
                    name: "Main Bathroom".to_string(),
                },
                AreaInfo {
                    id: AreaId("bathroom_two".to_string()),
                    name: " main bathroom ".to_string(),
                },
                AreaInfo {
                    id: AreaId("ensuite".to_string()),
                    name: "Ensuite".to_string(),
                },
            ],
            vec![],
        );

        assert_eq!(
            catalog.area_named(" ENSUITE ").unwrap().id().as_str(),
            "ensuite"
        );
        assert!(matches!(
            catalog.area_named("suite"),
            Err(Error::AreaNotFound { .. })
        ));
        match catalog.area_named("Main Bathroom") {
            Err(Error::AreaAmbiguous { candidates, .. }) => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates.iter().any(|id| id.as_str() == "bathroom_one"));
                assert!(candidates.iter().any(|id| id.as_str() == "bathroom_two"));
            }
            result => panic!("expected area ambiguity, got {result:?}"),
        }
    }

    #[test]
    fn current_state_metadata_takes_precedence_over_registry_metadata() {
        let entity_id = EntityId::new("sensor.temperature").unwrap();
        let state = crate::state::EntityState {
            entity_id: entity_id.clone(),
            state: "unavailable".to_string(),
            attributes: Map::from_iter([
                ("friendly_name".to_string(), json!("Live Name")),
                ("device_class".to_string(), json!("temperature")),
            ]),
            last_changed: "2026-01-01T00:00:00Z".to_string(),
            last_updated: "2026-01-01T00:00:00Z".to_string(),
        };
        let catalog = EntityCatalog {
            client: HomeAssistantClient::with_seeded_states([state]),
            snapshot: Arc::new(CatalogSnapshot {
                areas: vec![],
                entities: vec![],
            }),
        };
        let snapshot = CatalogSnapshot::from_responses(
            vec![],
            EntityRegistryDisplayResponse {
                entities: vec![EntityRegistryDisplayEntry {
                    entity_id,
                    name: Some("Registry Name".to_string()),
                }],
            },
            &catalog.client.generation,
        )
        .unwrap();
        let catalog = EntityCatalog {
            snapshot: Arc::new(snapshot),
            ..catalog
        };

        let entity = catalog.entities().query().exactly_one().unwrap();
        assert_eq!(entity.name(), Some("Live Name"));
        assert_eq!(entity.device_class(), Some("temperature"));
        assert!(
            catalog
                .entities()
                .query()
                .device_class("temperature")
                .exactly_one()
                .is_ok()
        );
    }

    #[test]
    fn filters_compose_with_exact_identifier_and_normalized_name_matching() {
        let matches = set()
            .query()
            .domain("sensor")
            .device_class("temperature")
            .named("bathroom temperature")
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].entity_id().as_str(),
            "sensor.bathroom_temperature"
        );

        assert!(set().query().domain("Sensor").collect().is_empty());
        assert!(
            set()
                .query()
                .device_class("Temperature")
                .collect()
                .is_empty()
        );
    }

    #[test]
    fn device_class_set_matches_only_entities_with_listed_metadata() {
        let matches = set()
            .query()
            .device_class_in(["occupancy", "motion"])
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].entity_id().as_str(),
            "binary_sensor.bathroom_motion"
        );
    }

    #[test]
    fn exactly_one_distinguishes_no_match_and_ambiguity() {
        assert!(matches!(
            set().query().domain("switch").exactly_one(),
            Err(Error::EntityQueryNotFound { .. })
        ));

        match set().query().domain("sensor").exactly_one() {
            Err(Error::EntityQueryAmbiguous { query, candidates }) => {
                assert!(query.contains("domain"));
                assert_eq!(candidates.len(), 3);
                assert!(
                    candidates
                        .iter()
                        .any(|id| id.as_str() == "sensor.bathroom_temperature")
                );
            }
            result => panic!("expected ambiguity, got {result:?}"),
        }
    }

    #[test]
    fn typed_conversion_reuses_handle_domain_validation() {
        let switch = entity("switch.fan", Some("Fan"), None);
        assert!(switch.switch().is_ok());
        assert!(switch.binary_sensor().is_err());
        assert!(switch.sensor::<f64>().is_err());
    }
}
