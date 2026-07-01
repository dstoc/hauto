use super::*;
use serde_json::json;

#[test]
fn light_turn_on_validates_brightness_pct() {
    assert!(
        LightTurnOn {
            brightness_pct: Some(100),
            ..Default::default()
        }
        .validate()
        .is_ok()
    );

    assert!(
        LightTurnOn {
            brightness_pct: Some(101),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn light_service_payloads_include_entity_transition_rgb_and_brightness() {
    let entity_id = EntityId::new("light.office").unwrap();
    let payload = LightTurnOn {
        brightness_pct: Some(75),
        brightness: Some(128),
        transition: Some(Duration::from_millis(1500)),
        color_temp_kelvin: Some(2700),
        rgb_color: Some((1, 2, 3)),
        effect: Some("pulse".to_string()),
    }
    .into_service_data(&entity_id);

    assert_eq!(
        payload,
        json!({
            "entity_id": "light.office",
            "brightness_pct": 75,
            "brightness": 128,
            "transition": 1.5,
            "color_temp_kelvin": 2700,
            "rgb_color": [1, 2, 3],
            "effect": "pulse",
        })
    );

    let payload = LightTurnOff {
        transition: Some(Duration::from_secs(2)),
    }
    .into_service_data(&entity_id);
    assert_eq!(
        payload,
        json!({
            "entity_id": "light.office",
            "transition": 2.0,
        })
    );
}
