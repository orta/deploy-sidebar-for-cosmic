// SPDX-License-Identifier: GPL-3.0

//! Credentials, kept in the system keyring (Secret Service).

use keyring::Entry;

const SERVICE_NAME: &str = "com.github.orta.cosmic-deploy-sidebar";
const GRAFANA_TOKEN: &str = "grafana_token";
const RENDER_API_KEY: &str = "render_api_key";

/// The cosmic-deploys applet's entry, read so the Render key only has to be set up once.
const APPLET_SERVICE_NAME: &str = "com.github.orta.cosmic-deploys";

/// Store the Grafana service account token.
pub fn store_grafana_token(token: &str) -> Result<(), keyring::Error> {
    Entry::new(SERVICE_NAME, GRAFANA_TOKEN)?.set_password(token)
}

/// Store a Render API key for this app, overriding the applet's.
pub fn store_render_api_key(key: &str) -> Result<(), keyring::Error> {
    Entry::new(SERVICE_NAME, RENDER_API_KEY)?.set_password(key)
}

/// The Grafana token, from the keyring or `GRAFANA_TOKEN`.
pub fn grafana_token() -> Option<String> {
    read(SERVICE_NAME, GRAFANA_TOKEN).or_else(|| std::env::var("GRAFANA_TOKEN").ok())
}

/// The Render key: this app's own, then the cosmic-deploys applet's, then `RENDER_API_KEY`.
pub fn render_api_key() -> Option<String> {
    read(SERVICE_NAME, RENDER_API_KEY)
        .or_else(|| read(APPLET_SERVICE_NAME, RENDER_API_KEY))
        .or_else(|| std::env::var("RENDER_API_KEY").ok())
}

fn read(service: &str, user: &str) -> Option<String> {
    Entry::new(service, user)
        .ok()?
        .get_password()
        .ok()
        .filter(|s| !s.is_empty())
}
