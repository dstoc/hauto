use super::*;
use serde_json::{Map, json};

mod protocol;

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
