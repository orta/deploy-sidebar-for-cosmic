// SPDX-License-Identifier: GPL-3.0

//! Render API client: just enough to notice deploys.

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::sync::LazyLock;
use std::time::Duration;

const API_BASE: &str = "https://api.render.com/v1";

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployStatus {
    Building,
    Live,
    Failed,
    Canceled,
    /// A past deploy which a newer one replaced.
    Superseded,
}

#[derive(Debug, Clone)]
pub struct Deploy {
    pub id: String,
    pub status: DeployStatus,
    /// Short commit SHA.
    pub revision: Option<String>,
    pub commit_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub url: String,
}

impl Deploy {
    /// A one-line summary of the commit, preferring the PR title for merge commits.
    pub fn commit_summary(&self) -> Option<&str> {
        let message = self.commit_message.as_deref()?;
        let mut lines = message.lines().map(str::trim).filter(|l| !l.is_empty());
        let first = lines.next()?;
        if first.starts_with("Merge pull request") {
            Some(lines.next().unwrap_or(first))
        } else {
            Some(first)
        }
    }
}

/// A service on the Render account, for `list-services`.
#[derive(Debug, Clone, Deserialize)]
pub struct Service {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct DeployItem {
    deploy: RenderDeploy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderDeploy {
    id: String,
    status: String,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    commit: Option<RenderCommit>,
}

#[derive(Deserialize)]
struct RenderCommit {
    id: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ServiceItem {
    service: Service,
}

/// Fetch the newest deploy of a service.
pub async fn latest_deploy(api_key: &str, service_id: &str) -> Result<Option<Deploy>, String> {
    let url = format!("{API_BASE}/services/{service_id}/deploys?limit=1");
    let items: Vec<DeployItem> = get(api_key, &url).await?;

    Ok(items.into_iter().next().map(|item| {
        let d = item.deploy;
        Deploy {
            url: format!(
                "https://dashboard.render.com/web/{service_id}/deploys/{}",
                d.id
            ),
            id: d.id,
            status: parse_status(&d.status),
            revision: d
                .commit
                .as_ref()
                .and_then(|c| c.id.as_deref())
                .map(|sha| sha.chars().take(7).collect()),
            commit_message: d.commit.and_then(|c| c.message),
            created_at: d.created_at,
            finished_at: d.finished_at,
        }
    }))
}

/// Fetch the services on the account (first page only), sorted by name.
pub async fn services(api_key: &str) -> Result<Vec<Service>, String> {
    let url = format!("{API_BASE}/services?limit=100&suspended=not_suspended");
    let items: Vec<ServiceItem> = get(api_key, &url).await?;
    let mut services: Vec<Service> = items.into_iter().map(|i| i.service).collect();
    services.sort_by_key(|s| s.name.to_lowercase());
    Ok(services)
}

async fn get<T: serde::de::DeserializeOwned>(api_key: &str, url: &str) -> Result<T, String> {
    let response = CLIENT
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Render rejected the API key".into());
    }
    response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

fn parse_status(status: &str) -> DeployStatus {
    match status {
        "created"
        | "queued"
        | "build_in_progress"
        | "update_in_progress"
        | "pre_deploy_in_progress" => DeployStatus::Building,
        "build_failed" | "update_failed" | "pre_deploy_failed" => DeployStatus::Failed,
        "canceled" => DeployStatus::Canceled,
        "deactivated" => DeployStatus::Superseded,
        _ => DeployStatus::Live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses() {
        assert_eq!(parse_status("live"), DeployStatus::Live);
        assert_eq!(parse_status("deactivated"), DeployStatus::Superseded);
        assert_eq!(parse_status("update_in_progress"), DeployStatus::Building);
        assert_eq!(parse_status("build_failed"), DeployStatus::Failed);
    }
}
