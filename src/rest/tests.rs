use super::ReqwestRestStateTransport;
use crate::Error;

#[test]
fn reqwest_rest_transport_rejects_non_http_base_urls() {
    assert!(matches!(
        ReqwestRestStateTransport::new("ws://homeassistant.local/api", "token"),
        Err(Error::Connection(_))
    ));
}
