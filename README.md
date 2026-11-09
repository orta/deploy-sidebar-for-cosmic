# Deploy Sidebar for COSMIC

A sidebar for the COSMIC desktop. It appears in the top right when a watched Render service deploys, shows server health against the 30 minutes before the deploy, and fades away 10 minutes after the deploy goes live. If a metric still looks bad at that point, it stays until dismissed.

It sits alongside [cosmic-deploys](https://github.com/orta/cosmic-deploys), the panel applet, and reads the Render key that applet stores in the keyring.

## How it works

- Every 20s it fetches the newest deploy of each configured service from the Render API.
- When a deploy starts building, the sidebar shows it with the elapsed time.
- Once the deploy is live, each configured PromQL query runs through Grafana's `/api/ds/query` once a minute. Prometheus stays private, because Grafana proxies the queries.
- Each metric gets a sparkline (bars before the deploy are dimmed), its current value, and its change from the pre-deploy mean. A metric 25% worse than its baseline is a warning; 50% worse is bad. `floor` stops tiny values from being flagged.

## Setup

```bash
just install    # ~/.local/bin, a systemd user service, and a "Deploy Sidebar Settings" launcher entry
just settings   # or open "Deploy Sidebar Settings" from the launcher
```

In settings:

- **Grafana**: the URL and a service account token with the Viewer role. Grafana's GitHub sign-in doesn't apply to tokens. Then pick the Prometheus datasource and use **Test connection**.
- **Render**: the key from the cosmic-deploys applet is used if it's there. Otherwise paste one and use **Load services**.
- **Services**: choose each one's Render service and edit its metrics. **Test queries** shows each metric's current value.

Saving writes `~/.config/cosmic-deploy-sidebar/config.toml`, and a running sidebar reloads it within a few seconds. The file is still plain TOML; `config.example.toml` documents the fields, but saving from the window drops comments.

The same steps also work from the terminal: `init-config`, `set-grafana-token`, `set-render-key`, `list-services`, `check`.

## Development

```bash
just demo                                       # synthetic data, no network
cargo run -- --preview api                      # real data for api's latest deploy, as if it just went live
just logs
```

Only the `datasource_uid`, `grafana_url` and service IDs are Puzzmo-specific. The metrics are just PromQL in the config file.
