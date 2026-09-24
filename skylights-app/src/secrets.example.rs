//! Sanitized template for application secrets and configuration.
//!
//! Copy this file to `skylights-app/src/secrets.rs` (or let `build.rs` auto-provision it)
//! and populate with site-specific credentials. This schema is shared with the
//! sibling firmware projects (garagelight, garagedoorcontroller) so one
//! operator file set works across the fleet. Real values must never be committed.

pub const WIFI_SSID: &str = "CHANGE_ME";
pub const WIFI_PASSWORD: &str = "CHANGE_ME";

/// `mqtt://<ipv4>[:port]`; the port defaults to 1883. IPv4 literal only.
pub const MQTT_BROKER: &str = "mqtt://CHANGE_ME:1883";

pub const OTA_URL: &str = "https://CHANGE_ME/firmware";
pub const OTA_PROJECT: &str = "skylights";
/// HTTP Basic auth for the OTA server; leave both empty to send no credentials.
pub const OTA_USER: &str = "";
pub const OTA_PASSWORD: &str = "";
