//! Sanitized template for application secrets and configuration.
//!
//! Copy this file to `skylights-app/src/secrets.rs` (or let `build.rs` auto-provision it)
//! and populate with site-specific credentials.

pub const WIFI_SSID: &str = "YOUR_WIFI_SSID";
pub const WIFI_PASS: &str = "YOUR_WIFI_PASSWORD";

pub const MQTT_HOST: &str = "mqtt.example.com";
pub const MQTT_PORT: u16 = 1883;
pub const MQTT_USER: Option<&str> = None;
pub const MQTT_PASSWORD: Option<&str> = None;

pub const OTA_BASE_URL: &str = "https://firmware.example.com";
pub const OTA_PROJECT: &str = "skylights";
pub const OTA_BASIC_AUTH_USER: Option<&str> = Some("ota-user");
pub const OTA_BASIC_AUTH_PASS: Option<&str> = Some("secret-token");
