use super::*;
use serde_json::json;

#[test]
fn state_write_requires_object_attributes() {
    assert!(StateWrite::new("ok", json!({ "friendly_name": "Status" })).is_ok());
    assert!(StateWrite::new("bad", json!(["not", "object"])).is_err());
}
