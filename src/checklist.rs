// SPDX-License-Identifier: GPL-3.0

//! A testing checklist for a deploy, written by `claude -p` in the local clone that best matches
//! the deployed commit.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The most items to ask for; the sidebar shows each on one line.
pub const MAX_ITEMS: usize = 6;
const TIMEOUT: Duration = Duration::from_mins(5);

#[derive(Debug, Clone)]
pub struct Checklist {
    /// The folder name of the clone Claude ran in.
    pub clone: String,
    pub items: Vec<String>,
    /// Shown instead of items when there was nothing to check.
    pub note: Option<String>,
}

/// Pick a clone containing `to` and ask Claude what to test in `from..to`.
pub async fn generate(
    service: String,
    repos: Vec<PathBuf>,
    repo_path: Option<String>,
    from: Option<String>,
    to: String,
) -> Result<Checklist, String> {
    let dir = pick_clone(&repos, from.as_deref(), &to).await?;
    let clone = dir
        .file_name()
        .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
    if from.as_deref() == Some(to.as_str()) {
        return Ok(Checklist {
            clone,
            items: Vec::new(),
            note: Some("Same commit as the previous deploy".into()),
        });
    }
    let range = match &from {
        Some(from) => format!("{from}..{to}"),
        None => format!("{to}~1..{to}"),
    };
    let items = ask_claude(&dir, &prompt(&service, &range, repo_path.as_deref())).await?;
    Ok(Checklist {
        clone,
        note: items
            .is_empty()
            .then(|| "Nothing that needs a manual check".into()),
        items,
    })
}

/// Has the previous commit, has the deployed one checked out, is clean, fewest commits past it.
type Score = (bool, bool, bool, std::cmp::Reverse<u64>);

/// The clone with the deployed commit, preferring one that also has the previous commit and has
/// that commit checked out, or the fewest commits on top of it, with a clean working tree.
async fn pick_clone(repos: &[PathBuf], from: Option<&str>, to: &str) -> Result<PathBuf, String> {
    let mut best: Option<(Score, &PathBuf)> = None;
    for dir in repos {
        if !has_commit(dir, to).await {
            continue;
        }
        let has_from = match from {
            Some(from) => has_commit(dir, from).await,
            None => true,
        };
        let contains = git(dir, &["merge-base", "--is-ancestor", to, "HEAD"])
            .await
            .is_some();
        let ahead = git(dir, &["rev-list", "--count", &format!("{to}..HEAD")])
            .await
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(u64::MAX);
        let clean = git(dir, &["status", "--porcelain"])
            .await
            .is_some_and(|s| s.trim().is_empty());
        let score = (has_from, contains, clean, std::cmp::Reverse(ahead));
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, dir));
        }
    }
    best.map(|(_, dir)| dir.clone()).ok_or_else(|| {
        let short: String = to.chars().take(7).collect();
        format!("No clone has {short} yet; git fetch in one of them")
    })
}

async fn has_commit(dir: &Path, sha: &str) -> bool {
    git(dir, &["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .await
        .is_some()
}

/// Run git in `dir`, returning stdout when it succeeds.
async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn prompt(service: &str, range: &str, repo_path: Option<&str>) -> String {
    let focus = repo_path.map_or_else(String::new, |path| {
        format!(
            " The service lives in `{path}`; focus on changes that affect it, including shared \
             code it uses, and ignore changes that only affect other apps."
        )
    });
    format!(
        "The {service} service was just deployed to production. The deployed changes are \
         `git diff {range}` (`git log {range}` lists the commits).{focus} The working tree may \
         differ from what was deployed, so go by the diff.\n\n\
         Write a short checklist of what to check by hand in production now that this is live: \
         concrete user-facing flows, pages or endpoints the changes touch, most important first. \
         At most {MAX_ITEMS} items, each an imperative phrase under 60 characters. Skip generic \
         advice and anything automated tests would already cover. Return an empty list if \
         nothing needs a manual check."
    )
}

/// Run `claude -p` read-only in `dir` and return its checklist items.
async fn ask_claude(dir: &Path, prompt: &str) -> Result<Vec<String>, String> {
    let schema = format!(
        r#"{{"type":"object","properties":{{"todos":{{"type":"array","maxItems":{MAX_ITEMS},"items":{{"type":"string"}}}}}},"required":["todos"]}}"#
    );
    let child = Command::new("claude")
        .current_dir(dir)
        .args([
            "-p",
            prompt,
            "--output-format",
            "json",
            "--json-schema",
            &schema,
        ])
        .args(["--allowedTools", "Read", "Grep", "Glob"])
        .args(["Bash(git diff:*)", "Bash(git log:*)", "Bash(git show:*)"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The sidebar drops the task when a newer deploy replaces this one.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not run claude: {e}"))?;
    let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "claude took over 5 minutes".to_string())?
        .map_err(|e| e.to_string())?;
    parse(&output.stdout).ok_or_else(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        format!("claude failed ({}) {line}", output.status)
    })
}

/// Pull the items out of `claude -p --output-format json`.
fn parse(stdout: &[u8]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    if value["is_error"].as_bool() == Some(true) {
        return None;
    }
    let items = value["structured_output"]["todos"].as_array()?;
    Some(
        items
            .iter()
            .filter_map(|i| i.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .take(MAX_ITEMS)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_output() {
        let out =
            br#"{"is_error":false,"structured_output":{"todos":["Log in"," ","Play a daily"]}}"#;
        assert_eq!(parse(out).unwrap(), ["Log in", "Play a daily"]);
    }

    #[test]
    fn errors_have_no_items() {
        assert!(parse(br#"{"is_error":true,"result":"nope"}"#).is_none());
        assert!(parse(b"not json").is_none());
    }
}
