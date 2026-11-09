# CLAUDE.md

A COSMIC desktop sidebar that appears when a watched Render service deploys, shows server health against the pre-deploy baseline for 10 minutes, then fades out. See README.md for setup and usage.

## Layout

- `src/app.rs` - the libcosmic app: deploy state machine (`Phase`: Building / Live / Failed), layer surface lifecycle, fade animation, view
- `src/render.rs` - Render API, latest deploy per service
- `src/grafana.rs` - PromQL through Grafana's `/api/ds/query`; multi-series results are summed per timestamp
- `src/checklist.rs` - when a service lists `repos`, each new deploy runs a read-only `claude -p --json-schema` in the clone that best matches the deployed commit (has `prev..new`, has it checked out, clean, fewest commits past it). The range comes from `render::previous_commit`. Started when the deploy is first seen, kept across Building -> Live, aborted via an iced `Handle` (plus `kill_on_drop`) when the watch is dropped. Unticked items hold the card past its window. `cosmic-deploy-sidebar checklist <service>` runs it from the terminal
- `src/health.rs` - baseline comparison (`assess`), value formatting
- `src/config.rs` + `config.example.toml` - TOML config at `~/.config/cosmic-deploy-sidebar/config.toml`; the example is embedded and used by `init-config` and `--demo`
- `src/settings.rs` - the settings window (`cosmic-deploy-sidebar settings`), a normal libcosmic app with a main window, run as its own process. It saves the TOML config and keyring entries; the sidebar polls the config mtime every 3s and reloads (`App::reload_if_changed`), keeping watches unless the service IDs changed
- `src/cli.rs` - setup subcommands (`init-config`, `set-grafana-token`, `list-services`, `check`, ...)
- `src/keyring.rs` - Secret Service; falls back to the cosmic-deploys applet's Render key (`com.github.orta.cosmic-deploys` / `render_api_key`)

## Sibling project

`../deploys-for-cosmic` (cosmic-deploys) is the panel applet that also watches Render deploys. The user chose to keep this a separate app rather than extend it. `render.rs` follows its conventions. `Cargo.lock` was copied from it, and libcosmic is pinned to the same rev (`aabc8dc`), so `cargo build --offline` works from the shared cache. Keep the two on the same rev when bumping.

## Grafana auth

Puzzmo's Grafana uses GitHub OAuth for browser login. That doesn't affect the API: service account tokens are sent as `Bearer` and bypass it. A login proxy in front of Grafana would not, and `grafana::json_or_explain` turns its HTML response into a clear error. Datasources are listed from `/api/frontend/settings`, because a Viewer token can't read `/api/datasources`.

## libcosmic gotchas

- **Layer surface autosize does not work**: `size: None` in `SctkLayerSurfaceSettings` leaves the surface unmapped, and nothing appears. Pass an explicit size. `content_height()` estimates the layout height and `sync_surface()` calls `set_size` when it changes. If you change spacing or rows in the view, update `content_height()` to match.
- Layer surface commands live at `cosmic::iced_winit::commands::layer_surface`, settings at `cosmic::iced_runtime::platform_specific::wayland::layer_surface`.
- The app runs with `no_main_window(true)`; everything renders through `view_window`.
- iced has no whole-surface opacity, so the fade multiplies the alpha of every colour via `Palette::new(alpha)`. New widgets must take their colours from `Palette` or they won't fade. That's why the close control is a text link rather than an icon button.
- The view is only rebuilt after a message, so animation needs the 33ms `Frame` subscription. Don't gate the `Tick`/`Frame` subscriptions behind config being present; that bug once froze the setup notice partly transparent.

## Metrics facts (Puzzmo)

- Prometheus runs privately on Render (repo `puzzmo-com/prometheus`), so all queries must go through Grafana.
- `scrape_interval` is 1m: polling faster than 60s is pointless, and new instances take about a minute to appear after a deploy.
- The `rate5m` recording rules in that repo smooth over the first minutes after a deploy, so use raw `rate(...[2m])` in the config.
- api is job `puzzmo-api`; `http_request_duration_seconds` comes from `fastify-metrics` (labels `method`, `route`, `status_code`). Custom metrics are in `apps/api.puzzmo.com/src/lib/metrics.ts` in the app monorepo.
- puzzmo.com SSR is job `ssr`; the important metric is `ssr_documents_total{outcome}`. `timeout` and `error` outcomes still return HTTP 200, so status codes miss them.
- Not yet confirmed against real data: `nodejs_eventloop_lag_p99_seconds` naming and the `floor` values. Run `cargo run -- check` after config changes.

## Testing visually

- `cargo run -- settings` opens the settings window; it is a normal toplevel, so it lands on the focused monitor. In the demo screenshot it was at roughly x 4440-5440 of the 6880-wide capture.
- `cargo run -- --demo` shows synthetic deploys with no network: a live api deploy with a latency regression, and a building puzzmo.com deploy.
- The user's setup has two 3440x1440 monitors side by side (6880x1440 total). The surface opens on the focused output. To check rendering:

  ```bash
  (timeout 8 ./target/debug/cosmic-deploy-sidebar --demo >/dev/null 2>&1 &); sleep 4
  cosmic-screenshot --interactive=false --modal=false --notify=false --save-dir <dir>
  convert <shot>.png -crop 520x520+2920+0 a.png   # top right of the left monitor
  convert <shot>.png -crop 520x520+6360+0 b.png   # top right of the right monitor
  ```

  Use `command ls` in scripts (`ls` is aliased with colours). PIL is not installed; ImageMagick is.
- The sidebar pops up on the user's real desktop when run, so keep test runs short and tell them.

## Conventions

- `just check` runs clippy with `-W clippy::pedantic`; keep it clean. Format with `cargo fmt`.
- Edition 2024, GPL-3.0 SPDX headers, one-line doc comments on functions.

## macOS app (`macos/`)

A SwiftUI and AppKit port, built as a Swift package with no dependencies. `make` drives it (`just` isn't installed on the Mac). `make test` runs swift-testing; `make app` builds `build/Deploy Sidebar.app` and signs it ad hoc.

- `Sources/DeploySidebarCore` has no UI: `Config`, `TOML`, `Render`, `Grafana`, `Health`, `Keychain`, and `SidebarModel`, the port of `app.rs`'s state machine (`deployFetched`, `startWatch`, `sweep`). When deploy behaviour changes on one side, change the other to match.
- `Sources/DeploySidebar` holds `Main` (CLI dispatch, then the SwiftUI `App` with a `MenuBarExtra`), `AppDelegate`, `SidebarPanel`, `SidebarView`, `SettingsModel`/`SettingsView`, `CLI` and `LoginItem`.
- It uses the same config path and TOML format as Linux. `TOML.swift` is a hand-written parser for the subset the config needs, and `toTOML()` writes the file back.
- `ExampleConfig.swift` is generated from `../config.example.toml` by `make example`. `ConfigTests` fails if the two drift.
- Keychain service `io.orta.DeploySidebar`, accounts `grafana_token` / `render_api_key`. The Render key falls back to `io.orta.RenderVercelDeploy`, the menu bar app in `../render-vercel-deploy-info`. Ad-hoc signing means the Keychain prompts again after each rebuild; `SIGN_IDENTITY` overrides it. The identities on this Mac belong to Puzzmo Inc., so don't pick one without asking.
- `SidebarModel.clock` is injectable, and tests use it to step through the window and fade. Models built by `init(setup:)` never read the real config file.

### macOS gotchas

- The sidebar is a borderless `.nonactivatingPanel` `NSPanel` at `.floating` level. `SidebarHostingView` overrides `acceptsFirstMouse`, so links work on the first click without stealing focus.
- Unlike COSMIC, the panel sizes itself from SwiftUI: `onGeometryChange` reports the content size and `SidebarPanel.place` keeps the top right corner pinned. There is no `content_height()` estimate to maintain.
- Fading animates the window's `alphaValue`, so the COSMIC `Palette` alpha trick isn't needed; system colours are fine.
- A process started from a terminal can't take activation with `NSApp.activate()`, so the settings-only launch uses `activate(ignoringOtherApps:)`.
- SwiftUI's `App` already provides an Edit menu, so Cmd-V works in an accessory app. `installEditMenuIfNeeded` is a fallback.

### Testing the macOS app visually

- One built-in Retina screen, 1512x982 points (3024x1964 px). The panel sits at x 1120-1500, y 50.
- **Don't take full-screen screenshots**, because they capture the user's other apps. Find the app's window IDs with `CGWindowListCopyWindowInfo` (the panel is layer 3) and capture just that window with `screencapture -x -o -l <id>`.
- `make demo` runs in the foreground until quit, so background it and `pkill -f 'DeploySidebar --demo'` after capturing. Point `XDG_CONFIG_HOME` at a scratch directory to test config states without touching `~/.config`.
