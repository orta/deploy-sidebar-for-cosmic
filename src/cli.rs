// SPDX-License-Identifier: GPL-3.0

//! Setup subcommands, run instead of the sidebar.

use crate::config;
use crate::grafana::Grafana;
use crate::health;
use crate::keyring;
use crate::render;
use chrono::{TimeDelta, Utc};
use std::io::BufRead;
use std::time::Duration;

const HELP: &str = "cosmic-deploy-sidebar

Usage:
  cosmic-deploy-sidebar                      Run the sidebar (idle until a deploy)
  cosmic-deploy-sidebar --preview <service>  Show the sidebar now for a service's latest deploy
  cosmic-deploy-sidebar --demo               Show the sidebar with synthetic data, no network
  cosmic-deploy-sidebar settings             Open the settings window
  cosmic-deploy-sidebar init-config          Write the example config if there is none
  cosmic-deploy-sidebar set-grafana-token    Store a Grafana service account token (reads stdin)
  cosmic-deploy-sidebar set-render-key       Store a Render API key (reads stdin)
  cosmic-deploy-sidebar list-services        List Render services and their IDs
  cosmic-deploy-sidebar check                Run every configured query and print the results
  cosmic-deploy-sidebar checklist <service>  Ask Claude what to test for a service's latest deploy
";

/// Run a subcommand, returning the process exit code.
pub fn run(command: &str, args: &[String]) -> i32 {
    let result = match command {
        "init-config" => init_config(),
        "set-grafana-token" => read_secret("Grafana token")
            .and_then(|t| keyring::store_grafana_token(&t).map_err(|e| e.to_string())),
        "set-render-key" => read_secret("Render API key")
            .and_then(|k| keyring::store_render_api_key(&k).map_err(|e| e.to_string())),
        "list-services" => block_on(list_services()),
        "check" => block_on(check()),
        "checklist" => match args.first() {
            Some(name) => block_on(checklist(name)),
            None => Err("Usage: cosmic-deploy-sidebar checklist <service>".into()),
        },
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        _ => Err(format!("Unknown command `{command}`\n\n{HELP}")),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn init_config() -> Result<(), String> {
    let path = config::path();
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, config::EXAMPLE).map_err(|e| e.to_string())?;
    println!("Wrote {}", path.display());
    Ok(())
}

fn read_secret(name: &str) -> Result<String, String> {
    eprintln!("Paste the {name} and press enter:");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let secret = line.trim().to_string();
    if secret.is_empty() {
        return Err(format!("No {name} given"));
    }
    eprintln!("Stored the {name} in the keyring.");
    Ok(secret)
}

async fn list_services() -> Result<(), String> {
    let key = keyring::render_api_key().ok_or("No Render API key, run set-render-key")?;
    for service in render::services(&key).await? {
        println!("{}  {}", service.id, service.name);
    }
    Ok(())
}

/// Query the last 15 minutes of every metric, to confirm the config and token work.
async fn check() -> Result<(), String> {
    let config = config::load()?;
    let token = keyring::grafana_token().ok_or("No Grafana token, run set-grafana-token")?;
    let grafana = Grafana {
        url: config.grafana_url.clone(),
        token,
        datasource_uid: config.datasource_uid.clone(),
    };

    let to = Utc::now();
    let from = to - TimeDelta::minutes(15);
    for service in &config.services {
        println!("{}", service.name);
        let exprs: Vec<String> = service.metrics.iter().map(|m| m.expr.clone()).collect();
        let results = grafana
            .query_range(&exprs, from, to, Duration::from_mins(1))
            .await?;
        for (metric, result) in service.metrics.iter().zip(results) {
            let line = match result {
                Ok(points) => match points.last() {
                    Some((_, v)) => format!(
                        "{} ({} points)",
                        health::format(metric.unit, *v),
                        points.len()
                    ),
                    None => "no data".to_string(),
                },
                Err(e) => format!("error: {e}"),
            };
            println!("  {:<20} {line}", metric.label);
        }
    }
    Ok(())
}

/// Generate the checklist the sidebar would show for a service's latest deploy.
async fn checklist(name: &str) -> Result<(), String> {
    let config = config::load()?;
    let service = config
        .services
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("No service called {name} in the config"))?;
    if service.repos.is_empty() {
        return Err(format!("{name} has no `repos` in the config"));
    }
    let key = keyring::render_api_key().ok_or("No Render API key, run set-render-key")?;
    let deploy = render::latest_deploy(&key, &service.render_service_id)
        .await?
        .ok_or("The service has no deploys")?;
    let to = deploy.commit_id.ok_or("The latest deploy has no commit")?;
    let from = render::previous_commit(&key, &service.render_service_id, &deploy.id).await?;
    eprintln!(
        "Asking Claude about {}..{}",
        from.as_deref().map_or("?", |f| &f[..f.len().min(7)]),
        &to[..to.len().min(7)]
    );
    let checklist = crate::checklist::generate(
        service.name.clone(),
        service.repo_dirs(),
        service.repo_path.clone(),
        from,
        to,
    )
    .await?;
    println!("In {}:", checklist.clone);
    if let Some(note) = checklist.note {
        println!("  {note}");
    }
    for item in checklist.items {
        println!("  - {item}");
    }
    Ok(())
}

fn block_on(future: impl Future<Output = Result<(), String>>) -> Result<(), String> {
    tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())?
        .block_on(future)
}
