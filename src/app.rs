// SPDX-License-Identifier: GPL-3.0

//! The sidebar: idle until a watched service deploys, then a layer surface pinned to a screen
//! corner showing each metric against its pre-deploy baseline, fading out once the window ends.
//! Dragging it opens a drop target overlay on every output and re-anchors it where it lands.

use crate::checklist::Checklist;
use crate::config::{self, Config};
use crate::grafana::{Grafana, SeriesResult};
use crate::health::{self, Level, SeriesChange};
use crate::keyring;
use crate::placement::{self, Corner, Placement};
use crate::render::{self, Deploy, DeployStatus};
use chrono::{DateTime, TimeDelta, Utc};
use cosmic::cctk::sctk::reexports::client::protocol::wl_output::WlOutput;
use cosmic::iced::event::{self, PlatformSpecific, wayland};
use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Color, Length, Limits, Point, Subscription, mouse};
use cosmic::iced_runtime::platform_specific::wayland::layer_surface::{
    IcedMargin, IcedOutput, SctkLayerSurfaceSettings,
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
/// The gap between the sidebar and the screen edges.
const MARGIN: i32 = 12;
/// How far the pointer moves with the button down before it counts as a drag.
const DRAG_THRESHOLD: f32 = 6.0;
/// How long to wait after a drop for the pointer to land on an overlay, before using the estimate.
const DROP_WAIT: Duration = Duration::from_millis(400);
const FADE_IN: Duration = Duration::from_millis(250);
const FADE_OUT: Duration = Duration::from_secs(3);
/// How much history before the deploy the sparklines include, so the restart is visible.
const LEAD_IN: TimeDelta = TimeDelta::minutes(5);
const STEP: Duration = Duration::from_mins(1);
const SPARK_BARS: usize = 16;
const SPARK_HEIGHT: f32 = 18.0;
/// The most series listed under a metric that looks off.
const MAX_BREAKDOWN: usize = 2;
/// How far back the current value of each breakdown series is read from.
const BREAKDOWN_RECENT: TimeDelta = TimeDelta::minutes(5);

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
    /// Per metric index, the breakdown series that got worse, while the metric looks off.
    breakdowns: HashMap<usize, Vec<SeriesChange>>,
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

/// A connected screen, as reported by the compositor.
#[derive(Debug, Clone)]
pub struct Output {
    wl: WlOutput,
    /// The connector name, e.g. `DP-1`, which is what gets saved.
    name: Option<String>,
    position: (i32, i32),
    size: (i32, i32),
}

impl Output {
    fn contains(&self, x: f32, y: f32) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let (left, top, w, h) = (
            self.position.0 as f32,
            self.position.1 as f32,
            self.size.0 as f32,
            self.size.1 as f32,
        );
        x >= left && x < left + w && y >= top && y < top + h
    }

    /// The corner nearest a point given in global coordinates.
    fn corner_at(&self, x: f32, y: f32) -> Corner {
        #[allow(clippy::cast_precision_loss)]
        Corner::nearest(
            x - self.position.0 as f32,
            y - self.position.1 as f32,
            self.size.0 as f32,
            self.size.1 as f32,
        )
    }
}

/// Moving the sidebar to another corner.
enum Drag {
    Idle,
    /// The button went down on the sidebar's background at this surface-local point.
    Pressed(Point),
    /// Moved far enough: a drop target overlay is open on every output.
    Moving {
        /// Overlay surface per output, by index into `App::outputs`.
        overlays: Vec<(Id, usize)>,
        /// The output index and corner it would land in.
        target: Option<(usize, Corner)>,
        /// The button is up; waiting for the pointer to enter an overlay for an exact spot.
        released: bool,
    },
}

/// What the pointer did, on which surface.
#[derive(Debug, Clone, Copy)]
pub enum Pointer {
    Moved(Point),
    Pressed,
    Released,
    Left,
}

#[derive(Debug, Clone)]
pub enum OutputChange {
    /// Added or changed.
    Info(Output),
    Removed(WlOutput),
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
    /// The deploy whose card the pointer is over, which shows its links and holds it open.
    hovered: Option<String>,
    placement: Placement,
    /// The output name the current surface was opened on; `None` for the focused output.
    surface_output: Option<String>,
    outputs: Vec<Output>,
    /// Where the pointer is over the sidebar, in surface coordinates.
    cursor: Option<Point>,
    drag: Drag,
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
    BreakdownFetched {
        deploy_id: String,
        metric: usize,
        result: Result<Vec<SeriesChange>, String>,
    },
    Hover(String),
    Unhover(String),
    Pointer(Id, Pointer),
    Output(OutputChange),
    DropTimeout,
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
            hovered: None,
            placement: placement::load(),
            surface_output: None,
            outputs: Vec::new(),
            cursor: None,
            drag: Drag::Idle,
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

    fn view_window(&self, id: Id) -> Element<'_, Message> {
        if let Drag::Moving {
            overlays, target, ..
        } = &self.drag
            && let Some((_, output)) = overlays.iter().find(|(o, _)| *o == id)
        {
            return self.drop_overlay(*output, *target);
        }
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
        subscriptions.push(event::listen_with(|event, status, id| match event {
            event::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(Message::Pointer(id, Pointer::Moved(position)))
            }
            // Presses a link or checkbox handled are captured, so they don't start a drag.
            event::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if status == event::Status::Ignored =>
            {
                Some(Message::Pointer(id, Pointer::Pressed))
            }
            event::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                Some(Message::Pointer(id, Pointer::Released))
            }
            event::Event::Mouse(mouse::Event::CursorLeft) => {
                Some(Message::Pointer(id, Pointer::Left))
            }
            event::Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Output(
                change,
                wl,
            ))) => output_change(change, wl).map(Message::Output),
            _ => None,
        }));
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
                let breakdowns = self.fetch_breakdowns(&deploy_id);
                return Task::batch([self.sync_surface(), breakdowns]);
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
                return self.checklist_fetched(&deploy_id, result);
            }
            Message::BreakdownFetched {
                deploy_id,
                metric,
                result,
            } => return self.breakdown_fetched(&deploy_id, metric, result),
            Message::Hover(deploy_id) => return self.hover(deploy_id, true),
            Message::Unhover(deploy_id) => return self.hover(deploy_id, false),
            Message::Pointer(id, pointer) => return self.pointer(id, pointer),
            Message::Output(change) => return self.output_changed(change),
            Message::DropTimeout => return self.drop_timeout(),
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
                    checklist: demo_checklist(),
                    checklist_task: None,
                    breakdowns: demo_breakdowns(),
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
                    breakdowns: HashMap::new(),
                },
            ],
            notice: None,
            surface: None,
            surface_height: 0,
            shown_at: Instant::now(),
            fade_started: None,
            hovered: None,
            placement: placement::load(),
            surface_output: None,
            outputs: Vec::new(),
            cursor: None,
            drag: Drag::Idle,
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
            breakdowns: HashMap::new(),
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

    /// Track the card under the pointer, which grows to show its links.
    fn hover(&mut self, deploy_id: String, entered: bool) -> Task<cosmic::Action<Message>> {
        if entered {
            self.hovered = Some(deploy_id);
            self.fade_started = None;
        } else if self.hovered.as_deref() == Some(deploy_id.as_str()) {
            self.hovered = None;
        }
        self.sync_surface()
    }

    /// Store a breakdown, dropping it when nothing in it got worse.
    fn breakdown_fetched(
        &mut self,
        deploy_id: &str,
        metric: usize,
        result: Result<Vec<SeriesChange>, String>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) {
            match result {
                Ok(changes) if !changes.is_empty() => {
                    watch.breakdowns.insert(metric, changes);
                }
                Ok(_) => {
                    watch.breakdowns.remove(&metric);
                }
                Err(e) => eprintln!("Breakdown: {e}"),
            }
        }
        self.sync_surface()
    }

    /// Store Claude's checklist, or why there isn't one.
    fn checklist_fetched(
        &mut self,
        deploy_id: &str,
        result: Result<Checklist, String>,
    ) -> Task<cosmic::Action<Message>> {
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
        self.sync_surface()
    }

    /// For each metric that looks off and has a breakdown query, find the series behind it.
    fn fetch_breakdowns(&mut self, deploy_id: &str) -> Task<cosmic::Action<Message>> {
        let (Some(grafana), Some(config)) = (self.grafana.clone(), &self.config) else {
            return Task::none();
        };
        let Some(watch) = self.watches.iter_mut().find(|w| w.deploy.id == deploy_id) else {
            return Task::none();
        };
        let Some(service) = config.services.get(watch.service) else {
            return Task::none();
        };
        let started = watch.deploy.created_at;
        let baseline_from = started - TimeDelta::minutes(config.baseline_minutes);

        let mut tasks = Vec::new();
        for (i, metric) in service.metrics.iter().enumerate() {
            let current = watch
                .series
                .get(i)
                .and_then(|s| s.as_ref().ok())
                .and_then(|p| p.last())
                .map(|(_, v)| *v);
            let level = health::assess(metric, watch.baselines.get(i).copied().flatten(), current);
            let Some(expr) = metric.breakdown.clone().filter(|_| level >= Level::Warn) else {
                watch.breakdowns.remove(&i);
                continue;
            };
            let metric = metric.clone();
            let grafana = grafana.clone();
            let deploy_id = deploy_id.to_string();
            tasks.push(task(async move {
                let now = Utc::now();
                let result = async {
                    let before = grafana
                        .query_labelled(&expr, baseline_from, started, STEP)
                        .await?;
                    let after = grafana
                        .query_labelled(&expr, now - BREAKDOWN_RECENT, now, STEP)
                        .await?;
                    Ok::<_, String>(health::worst_series(
                        &metric,
                        &before,
                        &after,
                        MAX_BREAKDOWN,
                    ))
                }
                .await;
                Message::BreakdownFetched {
                    deploy_id,
                    metric: i,
                    result,
                }
            }));
        }
        Task::batch(tasks)
    }

    /// The service's Grafana dashboard over the baseline and the deploy, or Explore with its
    /// metrics when no dashboard is configured.
    fn grafana_link(&self, watch: &Watch) -> Option<String> {
        let config = self.config.as_ref()?;
        let service = config.services.get(watch.service)?;
        let base = config.grafana_url.trim_end_matches('/');
        let from = (watch.deploy.created_at - TimeDelta::minutes(config.baseline_minutes))
            .timestamp_millis();
        if let Some(dashboard) = service.grafana_dashboard.as_deref() {
            let url = if dashboard.starts_with("http") {
                dashboard.to_string()
            } else {
                format!("{base}/{}", dashboard.trim_start_matches('/'))
            };
            let separator = if url.contains('?') { '&' } else { '?' };
            return Some(format!("{url}{separator}from={from}&to=now"));
        }
        let datasource = serde_json::json!({ "type": "prometheus", "uid": config.datasource_uid });
        let queries: Vec<serde_json::Value> = service
            .metrics
            .iter()
            .enumerate()
            .map(|(i, m)| {
                serde_json::json!({
                    "refId": format!("Q{i}"),
                    "expr": m.expr,
                    "legendFormat": m.label,
                    "datasource": datasource,
                })
            })
            .collect();
        let panes = serde_json::json!({ "deploy": {
            "datasource": config.datasource_uid,
            "queries": queries,
            "range": { "from": from.to_string(), "to": "now" },
        }});
        let mut url = reqwest::Url::parse(&format!("{base}/explore")).ok()?;
        url.query_pairs_mut()
            .append_pair("schemaVersion", "1")
            .append_pair("orgId", "1")
            .append_pair("panes", &panes.to_string());
        Some(url.into())
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
        let hovered = self.hovered.clone();
        let finished = |w: &Watch| match w.phase {
            Phase::Live(at) => {
                now - at >= window
                    && hovered.as_deref() != Some(w.deploy.id.as_str())
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
            (true, None) => self.open_surface(height),
            (false, Some(id)) => {
                self.surface = None;
                self.fade_started = None;
                Task::batch([destroy_layer_surface(id), self.close_overlays()])
            }
            _ => Task::none(),
        }
    }

    /// Create the sidebar's layer surface in the saved corner and output.
    fn open_surface(&mut self, height: u32) -> Task<cosmic::Action<Message>> {
        let id = Id::unique();
        self.surface = Some(id);
        self.surface_height = height;
        self.shown_at = Instant::now();
        self.fade_started = None;
        self.cursor = None;
        let output = self.saved_output();
        self.surface_output = output.and_then(|i| self.outputs[i].name.clone());
        let corner = self.placement.corner;
        let (top, bottom) = if corner.is_top() {
            (MARGIN, 0)
        } else {
            (0, MARGIN)
        };
        let (left, right) = if corner.is_left() {
            (MARGIN, 0)
        } else {
            (0, MARGIN)
        };
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::None,
            anchor: (if corner.is_top() {
                Anchor::TOP
            } else {
                Anchor::BOTTOM
            }) | (if corner.is_left() {
                Anchor::LEFT
            } else {
                Anchor::RIGHT
            }),
            output: output.map_or(IcedOutput::Active, |i| {
                IcedOutput::Output(self.outputs[i].wl.clone())
            }),
            namespace: "deploy-sidebar".into(),
            margin: IcedMargin {
                top,
                right,
                bottom,
                left,
            },
            // libcosmic's autosize leaves layer surfaces unmapped, so size it explicitly.
            size: Some((Some(WIDTH_PX), Some(height))),
            exclusive_zone: 0,
            size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
            ..Default::default()
        })
    }

    /// Close the sidebar and open it again, to move it to the saved placement.
    fn reopen_surface(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(old) = self.surface.take() else {
            return Task::none();
        };
        let height = self.content_height();
        Task::batch([destroy_layer_surface(old), self.open_surface(height)])
    }

    /// The index of the saved output, when it is connected.
    fn saved_output(&self) -> Option<usize> {
        let name = self.placement.output.as_ref()?;
        self.outputs
            .iter()
            .position(|o| o.name.as_ref() == Some(name))
    }

    /// The output the sidebar is on, when that is known.
    fn current_output(&self) -> Option<usize> {
        match &self.surface_output {
            Some(name) => self
                .outputs
                .iter()
                .position(|o| o.name.as_ref() == Some(name)),
            // Opened on the focused output, which is only certain with one screen.
            None if self.outputs.len() == 1 => Some(0),
            None => None,
        }
    }

    fn output_changed(&mut self, change: OutputChange) -> Task<cosmic::Action<Message>> {
        // A drag's overlays refer to outputs by index, so don't shift them under it.
        let cancel = self.close_overlays();
        match change {
            OutputChange::Info(output) => {
                if let Some(existing) = self.outputs.iter_mut().find(|o| o.wl == output.wl) {
                    *existing = output;
                } else {
                    self.outputs.push(output);
                }
                // Startup opened on the focused output before the saved one was known.
                if self.surface.is_some()
                    && self.surface_output.is_none()
                    && self.saved_output().is_some()
                {
                    return Task::batch([cancel, self.reopen_surface()]);
                }
            }
            OutputChange::Removed(wl) => {
                let removed = self.outputs.iter().position(|o| o.wl == wl);
                if let Some(i) = removed {
                    let output = self.outputs.remove(i);
                    // The compositor closes surfaces on a removed output.
                    if self.surface.is_some()
                        && output.name.is_some()
                        && output.name == self.surface_output
                    {
                        return Task::batch([cancel, self.reopen_surface()]);
                    }
                }
            }
        }
        cancel
    }

    fn pointer(&mut self, id: Id, pointer: Pointer) -> Task<cosmic::Action<Message>> {
        let on_sidebar = self.surface == Some(id);
        let overlay = match &self.drag {
            Drag::Moving { overlays, .. } => {
                overlays.iter().find(|(o, _)| *o == id).map(|(_, i)| *i)
            }
            _ => None,
        };
        match pointer {
            Pointer::Moved(position) => {
                if on_sidebar {
                    self.cursor = Some(position);
                }
                match &mut self.drag {
                    Drag::Pressed(start) if on_sidebar => {
                        if position.distance(*start) > DRAG_THRESHOLD {
                            return self.start_drag(position);
                        }
                    }
                    Drag::Moving {
                        target, released, ..
                    } => {
                        if let Some(output) = overlay {
                            // Exact: overlays cover their whole output.
                            let o = &self.outputs[output];
                            #[allow(clippy::cast_precision_loss)]
                            let corner = Corner::nearest(
                                position.x,
                                position.y,
                                o.size.0 as f32,
                                o.size.1 as f32,
                            );
                            *target = Some((output, corner));
                            if *released {
                                let target = *target;
                                return self.finish_drag(target);
                            }
                        } else if on_sidebar && !*released {
                            // The sidebar keeps the pointer while the button is held, so
                            // estimate where that is on the desktop.
                            let estimate = self.estimate_target(position);
                            if let Drag::Moving { target, .. } = &mut self.drag
                                && estimate.is_some()
                            {
                                *target = estimate;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Pointer::Pressed => {
                if let (Drag::Idle, true, Some(at)) = (&self.drag, on_sidebar, self.cursor) {
                    self.drag = Drag::Pressed(at);
                } else if let Drag::Moving { target, .. } = self.drag
                    && overlay.is_some()
                {
                    // The release got lost somewhere; a click on an overlay drops it.
                    return self.finish_drag(target);
                }
            }
            Pointer::Released => match &mut self.drag {
                Drag::Pressed(_) => self.drag = Drag::Idle,
                Drag::Moving { released, .. } if !*released => {
                    *released = true;
                    return task(async {
                        tokio::time::sleep(DROP_WAIT).await;
                        Message::DropTimeout
                    });
                }
                _ => {}
            },
            Pointer::Left => {
                if on_sidebar && matches!(self.drag, Drag::Idle) {
                    self.cursor = None;
                }
            }
        }
        Task::none()
    }

    /// Where a pointer at this point on the sidebar would drop it, in global coordinates.
    fn estimate_target(&self, position: Point) -> Option<(usize, Corner)> {
        let current = &self.outputs[self.current_output()?];
        #[allow(clippy::cast_precision_loss)]
        let (x, y) = self.placement.corner.origin(
            (current.size.0 as f32, current.size.1 as f32),
            (WIDTH, self.surface_height as f32),
            MARGIN as f32,
        );
        #[allow(clippy::cast_precision_loss)]
        let global = (
            current.position.0 as f32 + x + position.x,
            current.position.1 as f32 + y + position.y,
        );
        let i = self
            .outputs
            .iter()
            .position(|o| o.contains(global.0, global.1))?;
        Some((i, self.outputs[i].corner_at(global.0, global.1)))
    }

    /// Open a transparent drop target overlay on every output.
    fn start_drag(&mut self, position: Point) -> Task<cosmic::Action<Message>> {
        if self.outputs.is_empty() {
            self.drag = Drag::Idle;
            return Task::none();
        }
        let overlays: Vec<(Id, usize)> =
            (0..self.outputs.len()).map(|i| (Id::unique(), i)).collect();
        let tasks: Vec<_> = overlays
            .iter()
            .map(|(id, i)| {
                get_layer_surface(SctkLayerSurfaceSettings {
                    id: *id,
                    layer: Layer::Overlay,
                    keyboard_interactivity: KeyboardInteractivity::None,
                    anchor: Anchor::all(),
                    output: IcedOutput::Output(self.outputs[*i].wl.clone()),
                    namespace: "deploy-sidebar-drop".into(),
                    // Stretched across the output by the anchors, panels included.
                    size: Some((None, None)),
                    exclusive_zone: -1,
                    size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                    ..Default::default()
                })
            })
            .collect();
        let target = self.estimate_target(position);
        self.drag = Drag::Moving {
            overlays,
            target,
            released: false,
        };
        Task::batch(tasks)
    }

    /// The pointer never reached an overlay after the drop, so go with the estimate.
    fn drop_timeout(&mut self) -> Task<cosmic::Action<Message>> {
        match self.drag {
            Drag::Moving {
                released: true,
                target,
                ..
            } => self.finish_drag(target),
            _ => Task::none(),
        }
    }

    /// Close the overlays, and move the sidebar if it landed somewhere.
    fn finish_drag(&mut self, target: Option<(usize, Corner)>) -> Task<cosmic::Action<Message>> {
        let close = self.close_overlays();
        let Some((output, corner)) = target else {
            return close;
        };
        let placement = Placement {
            corner,
            output: self.outputs[output].name.clone(),
        };
        if placement == self.placement && self.current_output() == Some(output) {
            return close;
        }
        if let Err(e) = placement::save(&placement) {
            eprintln!("Saving the sidebar position: {e}");
        }
        self.placement = placement;
        Task::batch([close, self.reopen_surface()])
    }

    fn close_overlays(&mut self) -> Task<cosmic::Action<Message>> {
        match std::mem::replace(&mut self.drag, Drag::Idle) {
            Drag::Moving { overlays, .. } => Task::batch(
                overlays
                    .into_iter()
                    .map(|(id, _)| destroy_layer_surface(id)),
            ),
            _ => Task::none(),
        }
    }

    /// One output's overlay during a drag: the four corners, with the one it would land in lit.
    fn drop_overlay(&self, output: usize, target: Option<(usize, Corner)>) -> Element<'_, Message> {
        let palette = Palette::new(1.0);
        #[allow(clippy::cast_precision_loss)]
        let height = (self.surface_height as f32)
            .min(
                self.outputs
                    .get(output)
                    .map_or(400.0, |o| o.size.1 as f32 / 2.0)
                    - 40.0,
            )
            .max(60.0);
        let ghost = |corner: Corner| -> Element<'_, Message> {
            let lit = target == Some((output, corner));
            let mut fill = palette.accent;
            fill.a = if lit { 0.3 } else { 0.06 };
            let border = if lit { palette.accent } else { palette.divider };
            widget::container(widget::Space::new(0, 0))
                .width(Length::Fixed(WIDTH))
                .height(Length::Fixed(height))
                .class(cosmic::theme::Container::custom(move |_| {
                    cosmic::iced::widget::container::Style {
                        background: Some(fill.into()),
                        border: cosmic::iced::Border {
                            radius: 12.0.into(),
                            width: if lit { 2.0 } else { 1.0 },
                            color: border,
                        },
                        ..Default::default()
                    }
                }))
                .into()
        };
        let row = |left: Corner, right: Corner| {
            widget::row::with_capacity(3)
                .push(ghost(left))
                .push(widget::horizontal_space())
                .push(ghost(right))
        };
        #[allow(clippy::cast_precision_loss)]
        widget::container(
            widget::column::with_capacity(3)
                .push(row(Corner::TopLeft, Corner::TopRight))
                .push(widget::vertical_space())
                .push(row(Corner::BottomLeft, Corner::BottomRight)),
        )
        .padding(MARGIN as f32)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::custom(|_| {
            cosmic::iced::widget::container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.15).into()),
                ..Default::default()
            }
        }))
        .into()
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
            if matches!(watch.phase, Phase::Live(_)) {
                #[allow(clippy::cast_precision_loss)]
                let routes = watch.breakdowns.values().map(Vec::len).sum::<usize>() as f32;
                height += routes * (18.0 + 6.0);
            }
            if self.hovered.as_deref() == Some(watch.deploy.id.as_str()) {
                height += LINKS_HEIGHT + 6.0;
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

    /// Links to Grafana and Render, shown while the pointer is over a card.
    fn links_row<'a>(&self, watch: &Watch, palette: &Palette) -> Element<'a, Message> {
        let mut links = widget::row::with_capacity(3).spacing(6);
        if let Some(url) = self.grafana_link(watch) {
            links = links.push(pill("Grafana ↗", Message::Open(url), palette));
        }
        links = links.push(pill(
            "Deploy log ↗",
            Message::Open(watch.deploy.url.clone()),
            palette,
        ));
        if let Some(service) = self
            .config
            .as_ref()
            .and_then(|c| c.services.get(watch.service))
        {
            let url = format!(
                "https://dashboard.render.com/web/{}/logs",
                service.render_service_id
            );
            links = links.push(pill("Logs ↗", Message::Open(url), palette));
        }
        links.into()
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
            .push(widget::text::caption(status).class(text_color(palette.text)))
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
                for change in watch.breakdowns.get(&i).into_iter().flatten() {
                    card = card.push(breakdown_row(metric, change, palette));
                }
            }
        }
        if !matches!(watch.checklist, ChecklistState::Off) {
            card = card.push(rule(palette.divider));
            for line in checklist_lines(watch, palette) {
                card = card.push(line);
            }
        }

        if self.hovered.as_deref() == Some(watch.deploy.id.as_str()) {
            card = card.push(self.links_row(watch, palette));
        }

        let background = palette.card;
        let card = widget::container(card)
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
            }));
        widget::mouse_area(card)
            .on_enter(Message::Hover(watch.deploy.id.clone()))
            .on_exit(Message::Unhover(watch.deploy.id.clone()))
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

/// A half-ticked checklist for the demo's api deploy.
fn demo_checklist() -> ChecklistState {
    ChecklistState::Ready(
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
    )
}

/// Route breakdowns for the demo's p95 latency, the metric it regresses.
fn demo_breakdowns() -> HashMap<usize, Vec<SeriesChange>> {
    HashMap::from([(
        1,
        vec![
            SeriesChange {
                name: "POST /graphql".into(),
                baseline: Some(0.21),
                current: 0.48,
                level: Level::Bad,
            },
            SeriesChange {
                name: "GET /calendar/:day".into(),
                baseline: None,
                current: 0.31,
                level: Level::Bad,
            },
        ],
    )])
}

/// A series under a metric that got worse: its labels, current value and change.
fn breakdown_row<'a>(
    metric: &config::Metric,
    change: &SeriesChange,
    palette: &Palette,
) -> Element<'a, Message> {
    let delta = change
        .baseline
        .filter(|b| b.abs() >= f64::EPSILON)
        .and_then(|b| health::delta(b, change.current))
        .unwrap_or_else(|| "new".to_string());
    let name = if change.name.is_empty() {
        "(no labels)".to_string()
    } else {
        truncate(&change.name, 34)
    };
    widget::row::with_capacity(3)
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            widget::text::caption(format!("  ↳ {name}"))
                .class(text_color(palette.muted))
                .width(Length::Fill),
        )
        .push(
            widget::text::caption(health::format(metric.unit, change.current))
                .class(text_color(palette.text))
                .width(Length::Fixed(62.0))
                .align_x(Alignment::End),
        )
        .push(
            widget::text::caption(delta)
                .class(text_color(palette.level(change.level)))
                .width(Length::Fixed(40.0))
                .align_x(Alignment::End),
        )
        .into()
}

/// The height of the row of link pills shown while hovering a card.
const LINKS_HEIGHT: f32 = 24.0;

/// A small rounded link, coloured from the palette so it fades.
fn pill<'a>(label: &'a str, message: Message, palette: &Palette) -> Element<'a, Message> {
    let background = palette.divider;
    let content = widget::container(widget::text::caption(label).class(text_color(palette.text)))
        .padding([3, 10])
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                background: Some(background.into()),
                border: cosmic::iced::Border {
                    radius: 12.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));
    widget::mouse_area(content)
        .on_press(message)
        .interaction(cosmic::iced::mouse::Interaction::Pointer)
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

/// The outputs the compositor reports, as `OutputChange`s.
fn output_change(change: wayland::OutputEvent, wl: WlOutput) -> Option<OutputChange> {
    let info = match change {
        wayland::OutputEvent::Removed => return Some(OutputChange::Removed(wl)),
        wayland::OutputEvent::Created(info) => info?,
        wayland::OutputEvent::InfoUpdate(info) => info,
    };
    Some(OutputChange::Info(Output {
        wl,
        name: info.name,
        position: info.logical_position.unwrap_or(info.location),
        size: info.logical_size?,
    }))
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
