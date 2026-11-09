// SPDX-License-Identifier: GPL-3.0

//! The sidebar: idle until a watched service deploys, then a layer surface pinned to the top
//! right showing each metric against its pre-deploy baseline, fading out once the window ends.

use crate::checklist::Checklist;
use crate::config::{self, Config};
use crate::grafana::{Grafana, SeriesResult};
use crate::health::{self, Level};
use crate::keyring;
use crate::render::{self, Deploy, DeployStatus};
use chrono::{DateTime, TimeDelta, Utc};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Color, Length, Limits, Subscription};
use cosmic::iced_runtime::platform_specific::wayland::layer_surface::{
    IcedMargin, SctkLayerSurfaceSettings,
};
use cosmic::iced_winit::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, destroy_layer_surface, get_layer_surface, set_size,
};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

const WIDTH: f32 = 380.0;
const WIDTH_PX: u32 = 380;
const FADE_IN: Duration = Duration::from_millis(250);
const FADE_OUT: Duration = Duration::from_secs(3);
/// How much history before the deploy the sparklines include, so the restart is visible.
const LEAD_IN: TimeDelta = TimeDelta::minutes(5);
const STEP: Duration = Duration::from_mins(1);
const SPARK_BARS: usize = 16;
const SPARK_HEIGHT: f32 = 18.0;

pub struct Flags {
    /// Show the sidebar now for this service's latest deploy, as if it just went live.
    pub preview: Option<String>,
    /// Show synthetic deploys from the example config, without any network access.
    pub demo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Building,
    Live(DateTime<Utc>),
    Failed,
}

/// A deploy being shown in the sidebar.
struct Watch {
    service: usize,
    deploy: Deploy,
    phase: Phase,
    /// The pre-deploy mean of each metric.
    baselines: Vec<Option<f64>>,
    series: Vec<SeriesResult>,
    /// Set when the whole metrics request failed, e.g. a bad token.
    error: Option<String>,
    /// Closed by the user. Still polled, so a failure or regression can bring it back.
    dismissed: bool,
    /// It already looked bad when dismissed, so only a fresh regression reopens it.
    dismissed_while_bad: bool,
    checklist: ChecklistState,
    /// Aborts the `claude -p` run when the watch is dropped.
    checklist_task: Option<cosmic::iced::task::Handle>,
}

/// The testing checklist Claude writes for a deploy, when the service lists repo clones.
enum ChecklistState {
    Off,
    Pending,
    Ready(Checklist, Vec<bool>),
    Failed(String),
}

impl ChecklistState {
    /// Whether the card should stay up past its window for this.
    fn holds(&self) -> bool {
        match self {
            ChecklistState::Pending => true,
            ChecklistState::Ready(_, done) => done.iter().any(|d| !d),
            ChecklistState::Off | ChecklistState::Failed(_) => false,
        }
    }

    /// Lines the checklist section takes in the card.
    fn lines(&self) -> usize {
        match self {
            ChecklistState::Off => 0,
            ChecklistState::Pending | ChecklistState::Failed(_) => 1,
            ChecklistState::Ready(c, _) => 1 + c.items.len().max(usize::from(c.note.is_some())),
        }
    }
}

pub struct App {
    core: cosmic::Core,
    config: Option<Config>,
    /// The config file's mtime when last read, to reload after the settings window saves.
    config_modified: Option<std::time::SystemTime>,
    render_key: Option<String>,
    grafana: Option<Grafana>,
    preview: Option<String>,
    /// The newest deploy ID seen per service, to spot new ones.
    last_seen: HashMap<usize, String>,
    watches: Vec<Watch>,
    /// A setup problem, shown in the sidebar until dismissed.
    notice: Option<String>,
    surface: Option<Id>,
    surface_height: u32,
    shown_at: Instant,
    fade_started: Option<Instant>,
}

#[derive(Debug, Clone)]
pub enum Message {
    PollRender,
    DeployFetched {
        service: usize,
        result: Result<Option<Deploy>, String>,
    },
    PollMetrics,
    MetricsFetched {
        deploy_id: String,
        baseline: bool,
        result: Result<Vec<SeriesResult>, String>,
    },
    Tick,
    Frame,
    Dismiss(String),
    DismissNotice,
    CheckConfig,
    OpenSettings,
    Open(String),
    ChecklistFetched {
        deploy_id: String,
        result: Result<Checklist, String>,
    },
    ToggleTodo(String, usize),
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = "com.github.orta.cosmic-deploy-sidebar";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, flags: Flags) -> (Self, Task<cosmic::Action<Message>>) {
        if flags.demo {
            return App::demo(core);
        }
        let setup = Setup::load();
        let mut app = App {
            core,
            config: setup.config,
            config_modified: config::modified(),
            render_key: setup.render_key,
            grafana: setup.grafana,
            preview: flags.preview,
            last_seen: HashMap::new(),
            watches: Vec::new(),
            notice: setup.notice,
            surface: None,
            surface_height: 0,
            shown_at: Instant::now(),
            fade_started: None,
        };
        let surface = app.sync_surface();
        (
            app,
            Task::batch([surface, task(async { Message::PollRender })]),
        )
    }

    fn view(&self) -> Element<'_, Message> {
        widget::Space::new(0, 0).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let alpha = self.alpha();
        let palette = Palette::new(alpha);
        let space = cosmic::theme::spacing();

        let mut column = widget::column::with_capacity(self.watches.len() + 1)
            .spacing(space.space_s)
            .width(Length::Fill);
        if let Some(notice) = &self.notice {
            column = column.push(Self::notice_card(notice, &palette));
        }
        for watch in self.visible_watches() {
            column = column.push(self.watch_card(watch, &palette));
        }

        let background = palette.background;
        let border = palette.divider;
        widget::container(column)
            .width(Length::Fixed(WIDTH))
            .padding(space.space_s)
            .class(cosmic::theme::Container::custom(move |_| {
                cosmic::iced::widget::container::Style {
                    background: Some(background.into()),
                    border: cosmic::iced::Border {
                        radius: 12.0.into(),
                        width: 1.0,
                        color: border,
                    },
                    ..Default::default()
                }
            }))
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();
        if let Some(config) = &self.config {
            if self.render_key.is_some() {
                let every = Duration::from_secs(config.render_poll_seconds.max(5));
                subscriptions.push(cosmic::iced::time::every(every).map(|_| Message::PollRender));
            }
            if self.live_watches().next().is_some() {
                let every = Duration::from_secs(config.metrics_poll_seconds.max(15));
                subscriptions.push(cosmic::iced::time::every(every).map(|_| Message::PollMetrics));
            }
        }
        // Demo mode has no config file to watch.
        if self.config_modified.is_some() || self.config.is_none() {
            subscriptions.push(
                cosmic::iced::time::every(Duration::from_secs(3)).map(|_| Message::CheckConfig),
            );
        }
        // Dismissed watches still need sweeping when nothing is on screen.
        if self.surface.is_some() || !self.watches.is_empty() {
            subscriptions
                .push(cosmic::iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        if self.is_animating() {
            subscriptions
                .push(cosmic::iced::time::every(Duration::from_millis(33)).map(|_| Message::Frame));
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::PollRender => return self.poll_render(),
            Message::DeployFetched { service, result } => match result {
                Ok(Some(deploy)) => return self.deploy_fetched(service, deploy),
                Ok(None) => {}
                Err(e) => eprintln!("Render: {e}"),
            },
            Message::PollMetrics => {
                let ids: Vec<String> = self.live_watches().map(|w| w.deploy.id.clone()).collect();
                return Task::batch(ids.iter().map(|id| self.fetch_metrics(id, false)));
            }
            Message::MetricsFetched {
                deploy_id,
                baseline,
                result,
            } => {
                let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) else {
                    return Task::none();
                };
                match result {
                    Ok(series) if baseline => {
                        watch.baselines = series
                            .iter()
                            .map(|s| s.as_ref().ok().and_then(|points| mean_of(points)))
                            .collect();
                    }
                    Ok(series) => {
                        watch.series = series;
                        watch.error = None;
                    }
                    Err(e) => watch.error = Some(e),
                }
                self.reopen_if_regressed(&deploy_id);
                return self.sync_surface();
            }
            Message::Tick => return self.sweep(),
            Message::Frame => {
                if self.fade_started.is_some_and(|t| t.elapsed() >= FADE_OUT) {
                    // Dismissed watches keep watching for regressions until their window ends.
                    self.watches.retain(|w| w.dismissed);
                    return self.sync_surface();
                }
            }
            Message::Dismiss(deploy_id) => {
                let metrics: Vec<&[config::Metric]> =
                    self.config.as_ref().map_or_else(Vec::new, |c| {
                        c.services.iter().map(|s| s.metrics.as_slice()).collect()
                    });
                if let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) {
                    let metrics = metrics.get(watch.service).copied().unwrap_or(&[]);
                    watch.dismissed = true;
                    watch.dismissed_while_bad =
                        watch.phase == Phase::Failed || worst_level(watch, metrics) == Level::Bad;
                }
                return self.sync_surface();
            }
            Message::DismissNotice => {
                self.notice = None;
                return self.sync_surface();
            }
            Message::CheckConfig => return self.reload_if_changed(),
            Message::OpenSettings => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("settings").spawn();
                }
            }
            Message::Open(url) => {
                let _ = open::that_detached(url);
            }
            Message::ChecklistFetched { deploy_id, result } => {
                if let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) {
                    watch.checklist_task = None;
                    watch.checklist = match result {
                        Ok(checklist) => {
                            let done = vec![false; checklist.items.len()];
                            ChecklistState::Ready(checklist, done)
                        }
                        Err(e) => {
                            eprintln!("Checklist: {e}");
                            ChecklistState::Failed(e)
                        }
                    };
                }
                return self.sync_surface();
            }
            Message::ToggleTodo(deploy_id, index) => {
                if let Some(ChecklistState::Ready(_, done)) = self
                    .watches
                    .iter_mut()
                    .find(|w| w.deploy.id == deploy_id)
                    .map(|w| &mut w.checklist)
                    && let Some(d) = done.get_mut(index)
                {
                    *d = !*d;
                }
            }
        }
        Task::none()
    }
}

impl App {
    /// Pick up a config saved by the settings window. Watches survive unless the services changed.
    fn reload_if_changed(&mut self) -> Task<cosmic::Action<Message>> {
        let modified = config::modified();
        if modified == self.config_modified {
            return Task::none();
        }
        self.config_modified = modified;

        let setup = Setup::load();
        let service_ids = |c: &Option<Config>| -> Vec<String> {
            c.iter()
                .flat_map(|c| c.services.iter().map(|s| s.render_service_id.clone()))
                .collect()
        };
        if service_ids(&self.config) != service_ids(&setup.config) {
            self.watches.clear();
            self.last_seen.clear();
        }
        self.config = setup.config;
        self.render_key = setup.render_key;
        self.grafana = setup.grafana;
        self.notice = setup.notice;
        Task::batch([self.sync_surface(), task(async { Message::PollRender })])
    }

    /// An app with no credentials, showing one live deploy with a latency regression and one
    /// still building.
    fn demo(core: cosmic::Core) -> (Self, Task<cosmic::Action<Message>>) {
        let config: Config = toml::from_str(config::EXAMPLE).expect("example config parses");
        let now = Utc::now();
        let live_at = now - TimeDelta::minutes(3);
        let started = live_at - TimeDelta::minutes(4);
        let deploy = |id: &str, message: &str| Deploy {
            id: id.into(),
            status: DeployStatus::Live,
            revision: Some("23675c8".into()),
            commit_id: None,
            commit_message: Some(message.into()),
            created_at: started,
            finished_at: Some(live_at),
            url: "https://dashboard.render.com".into(),
        };

        let api = &config.services[0].metrics;
        // Per metric: the pre-deploy level and the multiplier after it, p95 latency regressing.
        let shapes = [1.0, 1.9, 1.1, 1.0, 1.3, 1.05];
        let baselines: Vec<Option<f64>> = [140.0, 0.18, 0.02, 0.0, 0.03, 6.0e8]
            .into_iter()
            .map(Some)
            .collect();
        let series = baselines
            .iter()
            .zip(shapes)
            .take(api.len())
            .map(|(baseline, after)| {
                let baseline = baseline.unwrap_or_default();
                Ok((0..13)
                    .map(|minute| {
                        let time = started - LEAD_IN + TimeDelta::minutes(minute);
                        let wobble = 1.0
                            + 0.06 * (f64::from(i32::try_from(minute).unwrap_or(0)) * 1.7).sin();
                        let level = if time < live_at { 1.0 } else { after };
                        (time, baseline * level * wobble)
                    })
                    .collect())
            })
            .collect();

        let mut app = App {
            core,
            config: Some(config),
            config_modified: None,
            render_key: None,
            grafana: None,
            preview: None,
            last_seen: HashMap::new(),
            watches: vec![
                Watch {
                    service: 0,
                    deploy: deploy(
                        "dep-demo-api",
                        "Merge pull request #5871\n\nCalendar playlist",
                    ),
                    phase: Phase::Live(live_at),
                    baselines,
                    series,
                    error: None,
                    dismissed: false,
                    dismissed_while_bad: false,
                    checklist: ChecklistState::Ready(
                        Checklist {
                            clone: "app2".into(),
                            items: vec![
                                "Open a calendar playlist and play a puzzle".into(),
                                "Reorder a playlist and check it saves".into(),
                                "Load the calendar logged out".into(),
                            ],
                            note: None,
                        },
                        vec![true, false, false],
                    ),
                    checklist_task: None,
                },
                Watch {
                    service: 1,
                    deploy: Deploy {
                        status: DeployStatus::Building,
                        created_at: now - TimeDelta::seconds(95),
                        finished_at: None,
                        ..deploy("dep-demo-app", "Tidy SSR logging")
                    },
                    phase: Phase::Building,
                    baselines: Vec::new(),
                    series: Vec::new(),
                    error: None,
                    dismissed: false,
                    dismissed_while_bad: false,
                    checklist: ChecklistState::Pending,
                    checklist_task: None,
                },
            ],
            notice: None,
            surface: None,
            surface_height: 0,
            shown_at: Instant::now(),
            fade_started: None,
        };
        let surface = app.sync_surface();
        (app, surface)
    }

    fn poll_render(&self) -> Task<cosmic::Action<Message>> {
        let (Some(config), Some(key)) = (&self.config, &self.render_key) else {
            return Task::none();
        };
        Task::batch(config.services.iter().enumerate().map(|(service, s)| {
            let key = key.clone();
            let id = s.render_service_id.clone();
            task(async move {
                let result = render::latest_deploy(&key, &id).await;
                Message::DeployFetched { service, result }
            })
        }))
    }

    fn deploy_fetched(&mut self, service: usize, deploy: Deploy) -> Task<cosmic::Action<Message>> {
        let previous = self.last_seen.insert(service, deploy.id.clone());
        let name = self.service_name(service).to_string();

        if self.preview.as_deref() == Some(name.as_str()) {
            self.preview = None;
            return self.start_watch(service, deploy, Phase::Live(Utc::now()));
        }

        let existing = self
            .watches
            .iter()
            .position(|w| w.deploy.id == deploy.id)
            .map(|i| self.watches[i].phase);

        match (previous, existing, deploy.status) {
            // On startup only a deploy already under way is worth showing.
            (None, _, DeployStatus::Building) => self.start_watch(service, deploy, Phase::Building),
            (None, _, _) => Task::none(),
            (Some(prev), _, status) if prev != deploy.id => match status {
                DeployStatus::Building => self.start_watch(service, deploy, Phase::Building),
                DeployStatus::Failed => self.start_watch(service, deploy, Phase::Failed),
                // It went live between polls.
                DeployStatus::Live => {
                    let live_at = deploy.finished_at.unwrap_or_else(Utc::now);
                    self.start_watch(service, deploy, Phase::Live(live_at))
                }
                DeployStatus::Canceled | DeployStatus::Superseded => Task::none(),
            },
            (_, Some(Phase::Building), DeployStatus::Live) => {
                let live_at = deploy.finished_at.unwrap_or_else(Utc::now);
                self.start_watch(service, deploy, Phase::Live(live_at))
            }
            (_, Some(Phase::Building), DeployStatus::Failed) => {
                self.start_watch(service, deploy, Phase::Failed)
            }
            (_, Some(_), DeployStatus::Canceled | DeployStatus::Superseded) => {
                self.watches.retain(|w| w.deploy.id != deploy.id);
                self.sync_surface()
            }
            _ => Task::none(),
        }
    }

    /// Show a deploy, replacing anything already shown for its service.
    fn start_watch(
        &mut self,
        service: usize,
        deploy: Deploy,
        phase: Phase,
    ) -> Task<cosmic::Action<Message>> {
        let deploy_id = deploy.id.clone();
        let metric_count = self.metrics(service).len();
        // Keep baselines and the checklist across the Building -> Live transition, and a
        // dismissal unless it failed.
        let previous = self
            .watches
            .iter()
            .position(|w| w.deploy.id == deploy_id)
            .map(|i| self.watches.remove(i));
        let baselines = previous
            .as_ref()
            .map_or_else(|| vec![None; metric_count], |w| w.baselines.clone());
        let dismissed = phase != Phase::Failed && previous.as_ref().is_some_and(|w| w.dismissed);
        let (checklist, checklist_task, launch) = match previous {
            _ if phase == Phase::Failed => (ChecklistState::Off, None, None),
            Some(w) => (w.checklist, w.checklist_task, None),
            None => match self.start_checklist(service, &deploy) {
                Some((task, handle)) => (ChecklistState::Pending, Some(handle), Some(task)),
                None => (ChecklistState::Off, None, None),
            },
        };

        self.watches.retain(|w| w.service != service);
        self.watches.push(Watch {
            service,
            deploy,
            phase,
            baselines,
            series: (0..metric_count).map(|_| Ok(Vec::new())).collect(),
            error: None,
            dismissed,
            dismissed_while_bad: false,
            checklist,
            checklist_task,
        });
        self.watches.sort_by_key(|w| w.service);
        self.fade_started = None;

        let mut tasks = vec![self.sync_surface()];
        tasks.extend(launch);
        if matches!(phase, Phase::Live(_)) {
            tasks.push(self.fetch_metrics(&deploy_id, true));
            tasks.push(self.fetch_metrics(&deploy_id, false));
        }
        Task::batch(tasks)
    }

    /// Ask Claude for a testing checklist, when the service lists repo clones.
    fn start_checklist(
        &self,
        service: usize,
        deploy: &Deploy,
    ) -> Option<(Task<cosmic::Action<Message>>, cosmic::iced::task::Handle)> {
        let config = self.config.as_ref()?.services.get(service)?;
        let to = deploy.commit_id.clone()?;
        if config.repos.is_empty() {
            return None;
        }
        let key = self.render_key.clone();
        let service_id = config.render_service_id.clone();
        let name = config.name.clone();
        let repos = config.repo_dirs();
        let repo_path = config.repo_path.clone();
        let deploy_id = deploy.id.clone();
        let (task, handle) = task(async move {
            let from = match &key {
                Some(key) => render::previous_commit(key, &service_id, &deploy_id)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let result = crate::checklist::generate(name, repos, repo_path, from, to).await;
            Message::ChecklistFetched { deploy_id, result }
        })
        .abortable();
        Some((task, handle.abort_on_drop()))
    }

    /// Query every metric for a deploy: the pre-deploy baseline, or the window around it.
    fn fetch_metrics(&self, deploy_id: &str, baseline: bool) -> Task<cosmic::Action<Message>> {
        let (Some(grafana), Some(config)) = (self.grafana.clone(), &self.config) else {
            return Task::none();
        };
        let Some(watch) = self.watches.iter().find(|w| w.deploy.id == deploy_id) else {
            return Task::none();
        };
        let exprs: Vec<String> = self
            .metrics(watch.service)
            .iter()
            .map(|m| m.expr.clone())
            .collect();
        if exprs.is_empty() {
            return Task::none();
        }

        let started = watch.deploy.created_at;
        let (from, to) = if baseline {
            (
                started - TimeDelta::minutes(config.baseline_minutes),
                started,
            )
        } else {
            (started - LEAD_IN, Utc::now())
        };
        let deploy_id = deploy_id.to_string();
        task(async move {
            let result = grafana.query_range(&exprs, from, to, STEP).await;
            Message::MetricsFetched {
                deploy_id,
                baseline,
                result,
            }
        })
    }

    /// Drop watches whose window has passed, fading out when that leaves nothing to show.
    fn sweep(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(config) = &self.config else {
            return Task::none();
        };
        let window = TimeDelta::minutes(config.window_minutes);
        let now = Utc::now();
        let metrics: Vec<&[config::Metric]> = config
            .services
            .iter()
            .map(|s| s.metrics.as_slice())
            .collect();
        let finished = |w: &Watch| match w.phase {
            Phase::Live(at) => {
                now - at >= window
                    && worst_level(w, metrics[w.service]) < Level::Bad
                    && (w.dismissed || !w.checklist.holds())
            }
            // Nothing brings back a dismissed failure, so stop tracking it.
            Phase::Failed => w.dismissed,
            Phase::Building => false,
        };

        let mut visible = self.watches.iter().filter(|w| !w.dismissed).peekable();
        if self.notice.is_none() && visible.peek().is_some() && visible.all(finished) {
            self.fade_started.get_or_insert_with(Instant::now);
            return Task::none();
        }
        self.fade_started = None;
        self.watches.retain(|w| !finished(w));
        self.sync_surface()
    }

    /// Create, resize or destroy the layer surface to match what there is to show.
    fn sync_surface(&mut self) -> Task<cosmic::Action<Message>> {
        let wanted = self.notice.is_some() || self.visible_watches().next().is_some();
        let height = self.content_height();
        match (wanted, self.surface) {
            (true, Some(id)) if height != self.surface_height => {
                self.surface_height = height;
                set_size(id, Some(WIDTH_PX), Some(height))
            }
            (true, None) => {
                let id = Id::unique();
                self.surface = Some(id);
                self.surface_height = height;
                self.shown_at = Instant::now();
                self.fade_started = None;
                get_layer_surface(SctkLayerSurfaceSettings {
                    id,
                    layer: Layer::Top,
                    keyboard_interactivity: KeyboardInteractivity::None,
                    anchor: Anchor::TOP | Anchor::RIGHT,
                    namespace: "deploy-sidebar".into(),
                    margin: IcedMargin {
                        top: 12,
                        right: 12,
                        bottom: 0,
                        left: 0,
                    },
                    // libcosmic's autosize leaves layer surfaces unmapped, so size it explicitly.
                    size: Some((Some(WIDTH_PX), Some(height))),
                    exclusive_zone: 0,
                    size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                    ..Default::default()
                })
            }
            (false, Some(id)) => {
                self.surface = None;
                self.fade_started = None;
                destroy_layer_surface(id)
            }
            _ => Task::none(),
        }
    }

    /// The height the view will lay out at, mirroring the spacing used in `view_window`.
    fn content_height(&self) -> u32 {
        let padding = f32::from(cosmic::theme::spacing().space_s);
        let mut height = padding * 2.0;
        let mut cards = 0.0;
        if let Some(notice) = &self.notice {
            #[allow(clippy::cast_precision_loss)]
            let lines = (notice.chars().count() as f32 / 44.0).ceil();
            height += 24.0 + lines * 20.0 + 26.0;
            cards += 1.0;
        }
        for watch in self.visible_watches() {
            // Header and commit line, inside the card's padding.
            height += 20.0 + 24.0 + 6.0 + 18.0;
            if matches!(watch.phase, Phase::Live(_)) {
                #[allow(clippy::cast_precision_loss)]
                let rows = self.metrics(watch.service).len() as f32;
                height += rows * (SPARK_HEIGHT + 6.0);
                if watch.error.is_some() {
                    height += 24.0;
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let lines = watch.checklist.lines() as f32;
            if lines > 0.0 {
                // A divider row, then one line per item.
                height += 6.0 + lines * (18.0 + 6.0);
            }
            cards += 1.0;
        }
        height += (cards - 1.0_f32).max(0.0) * padding;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let height = height.ceil() as u32;
        height
    }

    fn alpha(&self) -> f32 {
        let fade_in = (self.shown_at.elapsed().as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
        let fade_out = self.fade_started.map_or(1.0, |t| {
            1.0 - (t.elapsed().as_secs_f32() / FADE_OUT.as_secs_f32()).min(1.0)
        });
        fade_in.min(fade_out)
    }

    fn is_animating(&self) -> bool {
        self.surface.is_some() && (self.shown_at.elapsed() < FADE_IN || self.fade_started.is_some())
    }

    fn live_watches(&self) -> impl Iterator<Item = &Watch> {
        self.watches
            .iter()
            .filter(|w| matches!(w.phase, Phase::Live(_)))
    }

    /// Watches that have not been dismissed.
    fn visible_watches(&self) -> impl Iterator<Item = &Watch> {
        self.watches.iter().filter(|w| !w.dismissed)
    }

    /// Bring a dismissed watch back when a metric newly looks bad.
    fn reopen_if_regressed(&mut self, deploy_id: &str) {
        let Some(config) = &self.config else {
            return;
        };
        let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) else {
            return;
        };
        if !watch.dismissed {
            return;
        }
        let metrics = config
            .services
            .get(watch.service)
            .map_or(&[][..], |s| s.metrics.as_slice());
        if worst_level(watch, metrics) < Level::Bad {
            watch.dismissed_while_bad = false;
        } else if !watch.dismissed_while_bad {
            watch.dismissed = false;
            self.fade_started = None;
        }
    }

    fn service_name(&self, service: usize) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.services.get(service))
            .map_or("?", |s| s.name.as_str())
    }

    fn metrics(&self, service: usize) -> &[config::Metric] {
        self.config
            .as_ref()
            .and_then(|c| c.services.get(service))
            .map_or(&[], |s| s.metrics.as_slice())
    }

    fn notice_card<'a>(notice: &'a str, palette: &Palette) -> Element<'a, Message> {
        widget::column::with_capacity(2)
            .spacing(4)
            .push(widget::text::heading("Deploy sidebar").class(text_color(palette.warn)))
            .push(widget::text::body(notice).class(text_color(palette.text)))
            .push(
                widget::row::with_capacity(2)
                    .spacing(12)
                    .push(link("Open settings", palette.accent, Message::OpenSettings))
                    .push(link("Dismiss", palette.muted, Message::DismissNotice)),
            )
            .into()
    }

    fn watch_card<'a>(&'a self, watch: &'a Watch, palette: &Palette) -> Element<'a, Message> {
        let metrics = self.metrics(watch.service);
        let now = Utc::now();
        let (status, status_color) = match watch.phase {
            Phase::Building => (
                format!("deploying · {}", duration(now - watch.deploy.created_at)),
                palette.accent,
            ),
            Phase::Failed => ("deploy failed".to_string(), palette.bad),
            Phase::Live(at) => {
                let window =
                    TimeDelta::minutes(self.config.as_ref().map_or(10, |c| c.window_minutes));
                let left = window - (now - at);
                let level = worst_level(watch, metrics);
                let text = if left > TimeDelta::zero() {
                    format!("live · {} left", duration(left))
                } else if level == Level::Bad {
                    "live · held, something looks off".to_string()
                } else {
                    "live · held for the checklist".to_string()
                };
                (text, palette.level(level))
            }
        };

        let header = widget::row::with_capacity(4)
            .spacing(8)
            .align_y(Alignment::Center)
            .push(dot(status_color))
            .push(
                widget::text::heading(self.service_name(watch.service))
                    .class(text_color(palette.text)),
            )
            .push(widget::horizontal_space())
            .push(widget::text::caption(status).class(text_color(palette.muted)))
            .push(link(
                "✕",
                palette.muted,
                Message::Dismiss(watch.deploy.id.clone()),
            ));

        let commit = match (&watch.deploy.revision, watch.deploy.commit_summary()) {
            (Some(rev), Some(summary)) => format!("{rev} · {summary}"),
            (Some(rev), None) => rev.clone(),
            (None, Some(summary)) => summary.to_string(),
            (None, None) => "Open in Render".to_string(),
        };

        let mut card = widget::column::with_capacity(metrics.len() + 3)
            .spacing(6)
            .push(header)
            .push(link(
                truncate(&commit, 48),
                palette.muted,
                Message::Open(watch.deploy.url.clone()),
            ));

        if let Phase::Live(live_at) = watch.phase {
            if let Some(error) = &watch.error {
                card =
                    card.push(widget::text::caption(error.clone()).class(text_color(palette.bad)));
            }
            for (i, metric) in metrics.iter().enumerate() {
                let points = watch.series.get(i).and_then(|s| s.as_ref().ok());
                let error = watch.series.get(i).and_then(|s| s.as_ref().err());
                let baseline = watch.baselines.get(i).copied().flatten();
                card = card.push(metric_row(
                    metric, points, error, baseline, live_at, palette,
                ));
            }
        }
        if !matches!(watch.checklist, ChecklistState::Off) {
            card = card.push(rule(palette.divider));
            for line in checklist_lines(watch, palette) {
                card = card.push(line);
            }
        }

        let background = palette.card;
        widget::container(card)
            .padding(10)
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(move |_| {
                cosmic::iced::widget::container::Style {
                    background: Some(background.into()),
                    border: cosmic::iced::Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }))
            .into()
    }
}

/// One metric: label, sparkline, current value and its change from the baseline.
fn metric_row<'a>(
    metric: &'a config::Metric,
    points: Option<&Vec<(DateTime<Utc>, f64)>>,
    error: Option<&String>,
    baseline: Option<f64>,
    live_at: DateTime<Utc>,
    palette: &Palette,
) -> Element<'a, Message> {
    let current = points.and_then(|p| p.last()).map(|(_, v)| *v);
    let level = health::assess(metric, baseline, current);
    let color = palette.level(level);

    let value = match (current, error) {
        (Some(v), _) => health::format(metric.unit, v),
        (None, Some(_)) => "error".to_string(),
        (None, None) => "…".to_string(),
    };
    let delta = baseline
        .zip(current)
        .and_then(|(b, c)| health::delta(b, c))
        .unwrap_or_default();

    widget::row::with_capacity(4)
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            widget::text::caption(metric.label.as_str())
                .class(text_color(palette.text))
                .width(Length::Fill),
        )
        .push(sparkline(
            points.map_or(&[], Vec::as_slice),
            baseline,
            live_at,
            color,
            palette,
        ))
        .push(
            widget::text::caption(value)
                .class(text_color(palette.text))
                .width(Length::Fixed(62.0))
                .align_x(Alignment::End),
        )
        .push(
            widget::text::caption(delta)
                // Normal movement stays quiet; only a warning or problem is coloured.
                .class(text_color(if level >= Level::Warn {
                    color
                } else {
                    palette.muted
                }))
                .width(Length::Fixed(40.0))
                .align_x(Alignment::End),
        )
        .into()
}

/// Bars for the latest points, dimmed before the deploy went live, scaled with the baseline.
fn sparkline<'a>(
    points: &[(DateTime<Utc>, f64)],
    baseline: Option<f64>,
    live_at: DateTime<Utc>,
    color: Color,
    palette: &Palette,
) -> Element<'a, Message> {
    let shown = &points[points.len().saturating_sub(SPARK_BARS)..];
    let max = shown
        .iter()
        .map(|(_, v)| *v)
        .chain(baseline)
        .fold(0.0_f64, f64::max);

    let mut row = widget::row::with_capacity(SPARK_BARS)
        .spacing(2)
        .height(Length::Fixed(SPARK_HEIGHT))
        .align_y(Alignment::End);
    for _ in shown.len()..SPARK_BARS {
        row = row.push(widget::Space::new(Length::Fixed(3.0), Length::Fixed(1.0)));
    }
    for (time, value) in shown {
        #[allow(clippy::cast_possible_truncation)]
        let fraction = if max > 0.0 { (value / max) as f32 } else { 0.0 };
        let bar_color = if *time < live_at {
            palette.muted
        } else {
            color
        };
        row = row.push(bar(bar_color, 2.0 + fraction * (SPARK_HEIGHT - 2.0)));
    }
    row.into()
}

fn bar<'a>(color: Color, height: f32) -> Element<'a, Message> {
    widget::container(widget::Space::new(
        Length::Fixed(3.0),
        Length::Fixed(height),
    ))
    .class(cosmic::theme::Container::custom(move |_| {
        cosmic::iced::widget::container::Style {
            background: Some(color.into()),
            border: cosmic::iced::Border {
                radius: 1.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }))
    .into()
}

fn dot<'a>(color: Color) -> Element<'a, Message> {
    widget::container(widget::Space::new(Length::Fixed(8.0), Length::Fixed(8.0)))
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                background: Some(color.into()),
                border: cosmic::iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

fn link<'a>(label: impl Into<String>, color: Color, message: Message) -> Element<'a, Message> {
    widget::mouse_area(widget::text::caption(label.into()).class(text_color(color)))
        .on_press(message)
        .interaction(cosmic::iced::mouse::Interaction::Pointer)
        .into()
}

/// A one pixel horizontal line, coloured from the palette so it fades.
fn rule<'a>(color: Color) -> Element<'a, Message> {
    widget::container(widget::Space::new(Length::Fill, 1))
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                background: Some(color.into()),
                ..Default::default()
            }
        }))
        .into()
}

/// The checklist section: a status line, then one tickable line per item.
fn checklist_lines<'a>(watch: &'a Watch, palette: &Palette) -> Vec<Element<'a, Message>> {
    let caption = |text: String, color: Color| -> Element<'a, Message> {
        widget::text::caption(text).class(text_color(color)).into()
    };
    match &watch.checklist {
        ChecklistState::Off => Vec::new(),
        ChecklistState::Pending => {
            vec![caption("Asking Claude what to test…".into(), palette.muted)]
        }
        ChecklistState::Failed(e) => vec![caption(
            truncate(&format!("No checklist: {e}"), 56),
            palette.muted,
        )],
        ChecklistState::Ready(checklist, done) => {
            let left = done.iter().filter(|d| !**d).count();
            let heading = if checklist.items.is_empty() {
                format!("To test · {}", checklist.clone)
            } else {
                format!("To test · {left} left · {}", checklist.clone)
            };
            let mut lines = vec![caption(heading, palette.muted)];
            if let Some(note) = &checklist.note {
                lines.push(caption(note.clone(), palette.text));
            }
            for (i, item) in checklist.items.iter().enumerate() {
                let ticked = done.get(i).copied().unwrap_or(false);
                let (mark, color) = if ticked {
                    ("☑", palette.muted)
                } else {
                    ("☐", palette.text)
                };
                lines.push(link(
                    format!("{mark}  {}", truncate(item, 52)),
                    color,
                    Message::ToggleTodo(watch.deploy.id.clone(), i),
                ));
            }
            lines
        }
    }
}

/// The most severe level across a watch's metrics.
fn worst_level(watch: &Watch, metrics: &[config::Metric]) -> Level {
    metrics
        .iter()
        .enumerate()
        .map(|(i, metric)| {
            let current = watch
                .series
                .get(i)
                .and_then(|s| s.as_ref().ok())
                .and_then(|p| p.last())
                .map(|(_, v)| *v);
            health::assess(metric, watch.baselines.get(i).copied().flatten(), current)
        })
        .max()
        .unwrap_or(Level::Unknown)
}

/// Theme colours with the current fade applied, since iced has no whole-surface opacity.
struct Palette {
    background: Color,
    card: Color,
    divider: Color,
    text: Color,
    muted: Color,
    accent: Color,
    ok: Color,
    warn: Color,
    bad: Color,
}

impl Palette {
    fn new(alpha: f32) -> Self {
        let theme = cosmic::theme::active();
        let c = theme.cosmic();
        let fade = |color: Color| Color {
            a: color.a * alpha,
            ..color
        };
        let mut muted = Color::from(c.background.on);
        muted.a = 0.55;
        Palette {
            background: fade(Color::from(c.background.base)),
            card: fade(Color::from(c.background.component.base)),
            divider: fade(Color::from(c.background.divider)),
            text: fade(Color::from(c.background.on)),
            muted: fade(muted),
            accent: fade(Color::from(c.accent.base)),
            ok: fade(Color::from(c.success.base)),
            warn: fade(Color::from(c.warning.base)),
            bad: fade(Color::from(c.destructive.base)),
        }
    }

    fn level(&self, level: Level) -> Color {
        match level {
            Level::Unknown => self.muted,
            Level::Ok => self.ok,
            Level::Warn => self.warn,
            Level::Bad => self.bad,
        }
    }
}

fn text_color(color: Color) -> cosmic::theme::Text {
    cosmic::theme::Text::Color(color)
}

fn mean_of(points: &[(DateTime<Utc>, f64)]) -> Option<f64> {
    let values: Vec<f64> = points.iter().map(|(_, v)| *v).collect();
    health::mean(&values)
}

fn duration(delta: TimeDelta) -> String {
    let seconds = delta.num_seconds().max(0);
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    }
}

/// Run a future and deliver its output as an app message.
fn task(future: impl Future<Output = Message> + Send + 'static) -> Task<cosmic::Action<Message>> {
    cosmic::task::future(future).map(cosmic::Action::App)
}

/// Config and credentials, plus the first problem stopping the sidebar from working.
struct Setup {
    config: Option<Config>,
    render_key: Option<String>,
    grafana: Option<Grafana>,
    notice: Option<String>,
}

impl Setup {
    fn load() -> Self {
        let (config, mut notice) = match config::load() {
            Ok(config) => (Some(config), None),
            Err(e) => (None, Some(e)),
        };
        let render_key = keyring::render_api_key();
        let grafana_token = keyring::grafana_token();
        if notice.is_none() && render_key.is_none() {
            notice = Some("No Render API key yet.".into());
        }
        if notice.is_none() && grafana_token.is_none() {
            notice = Some("No Grafana token yet.".into());
        }
        let grafana = config
            .as_ref()
            .zip(grafana_token)
            .map(|(c, token)| Grafana {
                url: c.grafana_url.clone(),
                token,
                datasource_uid: c.datasource_uid.clone(),
            });
        Setup {
            config,
            render_key,
            grafana,
            notice,
        }
    }
}
