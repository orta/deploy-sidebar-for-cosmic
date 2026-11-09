// SPDX-License-Identifier: GPL-3.0

//! The TOML config describing which services to watch and what to show for each.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const EXAMPLE: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Config {
    pub grafana_url: String,
    pub datasource_uid: String,
    #[serde(default = "default_window_minutes")]
    pub window_minutes: i64,
    #[serde(default = "default_baseline_minutes")]
    pub baseline_minutes: i64,
    #[serde(default = "default_render_poll_seconds")]
    pub render_poll_seconds: u64,
    #[serde(default = "default_metrics_poll_seconds")]
    pub metrics_poll_seconds: u64,
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Service {
    pub name: String,
    pub render_service_id: String,
    #[serde(default)]
    pub metrics: Vec<Metric>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Metric {
    pub label: String,
    pub expr: String,
    pub unit: Unit,
    pub worse: Direction,
    /// Values below this are never flagged, however they compare to the baseline.
    #[serde(default)]
    pub floor: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Count,
    PerSecond,
    Seconds,
    Bytes,
    Ratio,
}

/// The direction in which a metric moving means trouble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Higher,
    Lower,
}

/// Where the config lives: `$XDG_CONFIG_HOME/cosmic-deploy-sidebar/config.toml`.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("cosmic-deploy-sidebar").join("config.toml")
}

/// Read and validate the config, describing what is wrong when it can't be used.
pub fn load() -> Result<Config, String> {
    let path = path();
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Could not read {}: {e}. Open settings with `cosmic-deploy-sidebar settings`.",
            path.display()
        )
    })?;
    let config: Config =
        toml::from_str(&text).map_err(|e| format!("{} is invalid: {e}", path.display()))?;
    config.validate()?;
    Ok(config)
}

/// The saved config for editing, or the example with its placeholders blanked when there is none.
pub fn load_for_editing() -> Result<Config, String> {
    let path = path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        return toml::from_str(&text).map_err(|e| format!("{} is invalid: {e}", path.display()));
    }
    let mut config: Config = toml::from_str(EXAMPLE).map_err(|e| e.to_string())?;
    config.grafana_url.clear();
    config.datasource_uid.clear();
    for service in &mut config.services {
        if service.render_service_id == PLACEHOLDER {
            service.render_service_id.clear();
        }
    }
    Ok(config)
}

/// Write the config, replacing any comments (the example config documents every field).
pub fn save(config: &Config) -> Result<(), String> {
    let path = path();
    let body = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = format!(
        "# Written by `cosmic-deploy-sidebar settings`. Field docs: config.example.toml in the repo.\n\n{body}"
    );
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// When the config file last changed, to notice edits from the settings window.
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

const PLACEHOLDER: &str = "REPLACE_ME";

impl Config {
    /// Check the fields a running sidebar can't do without.
    pub fn validate(&self) -> Result<(), String> {
        let missing = |value: &str| value.trim().is_empty() || value == PLACEHOLDER;
        if missing(&self.grafana_url) {
            return Err("Set the Grafana URL in settings".into());
        }
        if missing(&self.datasource_uid) {
            return Err("Pick a Prometheus datasource in settings".into());
        }
        if let Some(service) = self.services.iter().find(|s| missing(&s.render_service_id)) {
            return Err(format!(
                "Pick a Render service for {} in settings",
                service.name
            ));
        }
        Ok(())
    }
}

fn default_window_minutes() -> i64 {
    10
}

fn default_baseline_minutes() -> i64 {
    30
}

fn default_render_poll_seconds() -> u64 {
    20
}

fn default_metrics_poll_seconds() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let config: Config = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(config.window_minutes, 10);
        assert_eq!(config.services.len(), 2);
        assert!(config.services.iter().all(|s| !s.metrics.is_empty()));
    }

    #[test]
    fn config_round_trips_through_toml() {
        let config: Config = toml::from_str(EXAMPLE).unwrap();
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }
}
