// SPDX-License-Identifier: GPL-3.0

//! Runs Prometheus queries through Grafana's `/api/ds/query`, so Prometheus never needs to be reachable.

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::Duration;

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_default()
});

/// Where to send queries, cloned into each request task.
#[derive(Debug, Clone)]
pub struct Grafana {
    pub url: String,
    pub token: String,
    pub datasource_uid: String,
}

/// One query's result: points sorted by time, or why it failed.
pub type SeriesResult = Result<Vec<(DateTime<Utc>, f64)>, String>;

/// Each series of a query kept apart, named by its label values (e.g. "POST /graphql").
pub type Labelled = Vec<(String, Vec<(DateTime<Utc>, f64)>)>;

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    results: HashMap<String, QueryResult>,
}

#[derive(Deserialize)]
struct QueryResult {
    #[serde(default)]
    frames: Vec<Frame>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct Frame {
    #[serde(default)]
    schema: Option<Schema>,
    data: FrameData,
}

#[derive(Deserialize)]
struct Schema {
    #[serde(default)]
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct FrameData {
    #[serde(default)]
    values: Vec<Vec<Value>>,
}

impl Grafana {
    /// Run every expression over `from..to`, returning one result per expression in order.
    pub async fn query_range(
        &self,
        exprs: &[String],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<SeriesResult>, String> {
        let response = self.post_queries(exprs, from, to, step).await?;
        Ok((0..exprs.len())
            .map(|i| match response.results.get(&ref_id(i)) {
                None => Err("No result".to_string()),
                Some(QueryResult {
                    error: Some(error), ..
                }) => Err(error.clone()),
                Some(result) => Ok(merge_frames(&result.frames)),
            })
            .collect())
    }

    /// Run one expression over `from..to`, keeping each series and its labels.
    pub async fn query_labelled(
        &self,
        expr: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        step: Duration,
    ) -> Result<Labelled, String> {
        let response = self
            .post_queries(&[expr.to_string()], from, to, step)
            .await?;
        match response.results.get(&ref_id(0)) {
            None => Err("No result".to_string()),
            Some(QueryResult {
                error: Some(error), ..
            }) => Err(error.clone()),
            Some(result) => Ok(result.frames.iter().map(labelled_frame).collect()),
        }
    }

    async fn post_queries(
        &self,
        exprs: &[String],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        step: Duration,
    ) -> Result<Response, String> {
        let step_ms = u64::try_from(step.as_millis()).unwrap_or(60_000);
        let queries: Vec<Value> = exprs
            .iter()
            .enumerate()
            .map(|(i, expr)| {
                json!({
                    "refId": ref_id(i),
                    "datasource": { "uid": self.datasource_uid, "type": "prometheus" },
                    "expr": expr,
                    "range": true,
                    "instant": false,
                    "intervalMs": step_ms,
                    "maxDataPoints": 500,
                })
            })
            .collect();
        let body = json!({
            "queries": queries,
            "from": from.timestamp_millis().to_string(),
            "to": to.timestamp_millis().to_string(),
        });

        let url = format!("{}/api/ds/query", self.url.trim_end_matches('/'));
        let response = CLIENT
            .post(url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        // A failing query still answers with per-refId errors, so only bail on auth problems.
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!("Grafana rejected the token ({status})"));
        }
        json_or_explain(response).await
    }
}

/// A Prometheus datasource the token can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datasource {
    pub name: String,
    pub uid: String,
}

#[derive(Deserialize)]
struct FrontendSettings {
    #[serde(default)]
    datasources: HashMap<String, FrontendDatasource>,
}

#[derive(Deserialize)]
struct FrontendDatasource {
    uid: String,
    #[serde(rename = "type")]
    kind: String,
}

impl Grafana {
    /// List Prometheus datasources. Uses the frontend settings, which a Viewer token can read,
    /// unlike `/api/datasources`.
    pub async fn prometheus_datasources(&self) -> Result<Vec<Datasource>, String> {
        let url = format!("{}/api/frontend/settings", self.url.trim_end_matches('/'));
        let response = CLIENT
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!("Grafana rejected the token ({status})"));
        }
        let settings: FrontendSettings = json_or_explain(response).await?;
        let mut datasources: Vec<Datasource> = settings
            .datasources
            .into_iter()
            .filter(|(_, d)| d.kind == "prometheus")
            .map(|(name, d)| Datasource { name, uid: d.uid })
            .collect();
        datasources.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(datasources)
    }

    /// Run a trivial query, to confirm the URL, token and datasource work together.
    pub async fn test(&self) -> Result<(), String> {
        let now = Utc::now();
        let results = self
            .query_range(
                &["vector(1)".to_string()],
                now - chrono::TimeDelta::minutes(5),
                now,
                Duration::from_mins(1),
            )
            .await?;
        match results.into_iter().next() {
            Some(Ok(points)) if !points.is_empty() => Ok(()),
            Some(Ok(_)) => Err("The datasource answered but returned no data".into()),
            Some(Err(e)) => Err(e),
            None => Err("No result".into()),
        }
    }
}

/// Parse JSON, explaining the usual cause when Grafana answers with a web page instead.
async fn json_or_explain<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let is_html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    if is_html {
        return Err(format!(
            "Got a web page instead of JSON from {}. A login proxy in front of Grafana would cause this.",
            response.url()
        ));
    }
    response
        .json()
        .await
        .map_err(|e| format!("Unexpected Grafana response ({status}): {e}"))
}

fn ref_id(index: usize) -> String {
    format!("Q{index}")
}

/// A frame's points, named by its value field's labels joined in key order.
fn labelled_frame(frame: &Frame) -> (String, Vec<(DateTime<Utc>, f64)>) {
    let name = frame
        .schema
        .as_ref()
        .and_then(|s| s.fields.get(1))
        .and_then(|f| f.labels.as_ref())
        .map(|labels| labels.values().cloned().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    (name, merge_frames(std::slice::from_ref(frame)))
}

/// Sum every frame's values per timestamp, so a query returning several series still plots as one.
fn merge_frames(frames: &[Frame]) -> Vec<(DateTime<Utc>, f64)> {
    let mut points: BTreeMap<i64, f64> = BTreeMap::new();
    for frame in frames {
        let [times, values, ..] = frame.data.values.as_slice() else {
            continue;
        };
        for (time, value) in times.iter().zip(values) {
            let (Some(time), Some(value)) = (time.as_i64(), value.as_f64()) else {
                continue;
            };
            if value.is_finite() {
                *points.entry(time).or_default() += value;
            }
        }
    }
    points
        .into_iter()
        .filter_map(|(ms, v)| DateTime::from_timestamp_millis(ms).map(|t| (t, v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_summed_and_nulls_skipped() {
        let response: Response = serde_json::from_value(json!({
            "results": {
                "Q0": { "status": 200, "frames": [
                    { "schema": {}, "data": { "values": [[1000, 2000, 3000], [1.0, null, 3.0]] } },
                    { "schema": {}, "data": { "values": [[1000, 2000], [0.5, 2.0]] } }
                ]},
                "Q1": { "status": 400, "error": "bad expr", "frames": [] }
            }
        }))
        .unwrap();

        let points = merge_frames(&response.results["Q0"].frames);
        let values: Vec<f64> = points.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, vec![1.5, 2.0, 3.0]);
        assert_eq!(response.results["Q1"].error.as_deref(), Some("bad expr"));
    }

    #[test]
    fn labelled_frames_are_named_by_label_values() {
        let frame: Frame = serde_json::from_value(json!({
            "schema": { "fields": [
                { "name": "Time" },
                { "name": "Value", "labels": { "route": "/graphql", "method": "POST" } }
            ]},
            "data": { "values": [[1000], [0.25]] }
        }))
        .unwrap();
        let (name, points) = labelled_frame(&frame);
        assert_eq!(name, "POST /graphql");
        assert_eq!(points.len(), 1);
    }
}
