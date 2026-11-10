# Deploy Sidebar for COSMIC

A sidebar for the COSMIC desktop. It appears in the top right when a watched Render service deploys (drag it by its background to any corner of any screen, and it stays there), shows server health against the 30 minutes before the deploy, and fades away 10 minutes after the deploy goes live. If a metric still looks bad at that point, it stays until dismissed. Closing a card with ✕ hides it until the next deploy, but the sidebar keeps watching that deploy and brings the card back if it fails or a metric starts to look bad.

It sits alongside [cosmic-deploys](https://github.com/orta/cosmic-deploys), the panel applet, and reads the Render key that applet stores in the keyring.

There is also a native macOS version, written in SwiftUI, in [`macos/`](macos/). See [macOS](#macos) below.

## How it works

- Every 20s it fetches the newest deploy of each configured service from the Render API.
- When a deploy starts building, the sidebar shows it with the elapsed time.
- Once the deploy is live, each configured PromQL query runs through Grafana's `/api/ds/query` once a minute. Prometheus stays private, because Grafana proxies the queries.
- Each metric gets a sparkline (bars before the deploy are dimmed), its current value, and its change from the pre-deploy mean. A metric 25% worse than its baseline is a warning; 50% worse is bad. `floor` stops tiny values from being flagged.
- A metric can have a `breakdown` query, the same metric split by a label like `route`. When the metric turns amber or red, the routes that got worst are listed under it (e.g. `↳ POST /graphql 480ms +129%`).
- Hovering a card shows links to the service's Grafana dashboard (or Explore, with the deploy's time range), the Render deploy log, and the service's Render logs.
- If a service lists `repos` (local clones of its repo), each deploy also runs a read-only `claude -p` in the clone that has the deployed commit, preferring one with it checked out and a clean tree. Claude reads the diff since the previous deploy and writes up to six things to test by hand, which appear under the stats as a checklist. The card stays up until they are ticked or closed. `cosmic-deploy-sidebar checklist <service>` prints the same list in the terminal. Set the clones and the service's folder in settings.

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

The same steps also work from the terminal: `init-config`, `set-grafana-token`, `set-render-key`, `list-services`, `check`, `checklist <service>`.

## Development

```bash
just demo                                       # synthetic data, no network
cargo run -- --preview api                      # real data for api's latest deploy, as if it just went live
just logs
```

Only the `datasource_uid`, `grafana_url` and service IDs are Puzzmo-specific. The metrics are just PromQL in the config file.

## macOS

The `macos/` folder is a SwiftUI and AppKit version of the same app. It reads the same config file at `~/.config/cosmic-deploy-sidebar/config.toml`, so one config works on both machines. It needs macOS 14 or later and the Xcode command line tools.

```bash
cd macos
make install    # builds "Deploy Sidebar.app" into ~/Applications and starts it
```

It runs as a menu bar app: the pulse icon opens a menu with **Settings…**, **Preview Latest Deploy** for each service, and **Quit**. The settings window is the same as on COSMIC, plus an **Open at login** switch. The sidebar floats at the top right of the screen the pointer is on until you drag it somewhere else, and never takes focus.

- Tokens are stored in the login Keychain under `io.orta.DeploySidebar`. With no Render key set, it uses the one the RenderVercelDeploy menu bar app saved (`io.orta.RenderVercelDeploy`). `GRAFANA_TOKEN` and `RENDER_API_KEY` also work.
- The app is signed ad hoc, so the Keychain asks again after each rebuild. Choose **Always Allow**, or build with a stable identity: `make install SIGN_IDENTITY="Apple Development: …"`.
- The CLI subcommands are the same, run from the binary inside the bundle: `~/Applications/Deploy\ Sidebar.app/Contents/MacOS/DeploySidebar check`.

```bash
make demo       # synthetic data, no network; quit it from its menu bar icon
make settings   # only the settings window
make queries    # run every configured query, like `check`
make test       # swift test
make logs       # log stream for io.orta.DeploySidebar
```
