// SPDX-License-Identifier: GPL-3.0

//! The settings window: connections, timing, and the services and metrics to watch.
//! Saving writes the config file, which a running sidebar notices and reloads.

use crate::config::{self, Config, Direction, Metric, Service, Unit};
use crate::grafana::{Datasource, Grafana, SeriesResult};
use crate::health;
use crate::keyring;
use crate::render;
use chrono::{TimeDelta, Utc};
use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

const UNITS: [(Unit, &str); 5] = [
    (Unit::Count, "Count"),
    (Unit::PerSecond, "Per second"),
    (Unit::Seconds, "Seconds"),
    (Unit::Bytes, "Bytes"),
    (Unit::Ratio, "Ratio"),
];
const UNIT_LABELS: [&str; 5] = ["Count", "Per second", "Seconds", "Bytes", "Ratio"];
const DIRECTIONS: [Direction; 2] = [Direction::Higher, Direction::Lower];
const DIRECTION_LABELS: [&str; 2] = ["Higher is worse", "Lower is worse"];
const RENDER_API_KEYS_URL: &str = "https://dashboard.render.com/u/settings#api-keys";

/// Something fetched on demand.
#[derive(Debug, Clone, Default)]
enum Load<T> {
    #[default]
    NotLoaded,
    Loading,
    Loaded(T),
    Failed(String),
}

pub struct SettingsApp {
    core: cosmic::Core,
    config: Config,
    /// Text being typed into number fields, parsed on save.
    window_minutes: String,
    baseline_minutes: String,
    floors: Vec<Vec<String>>,

    grafana_token_input: String,
    has_grafana_token: bool,
    datasources: Load<Vec<Datasource>>,
    datasource_labels: Vec<String>,
    grafana_test: Load<()>,

    render_key_input: String,
    has_render_key: bool,
    render_services: Load<Vec<render::Service>>,
    render_service_labels: Vec<String>,

    service_tests: HashMap<usize, Load<Vec<SeriesResult>>>,
    status: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    GrafanaUrl(String),
    GrafanaToken(String),
    LoadDatasources,
    DatasourcesLoaded(Result<Vec<Datasource>, String>),
    PickDatasource(usize),
    DatasourceUid(String),
    TestGrafana,
    GrafanaTested(Result<(), String>),

    RenderKey(String),
    LoadRenderServices,
    RenderServicesLoaded(Result<Vec<render::Service>, String>),

    WindowMinutes(String),
    BaselineMinutes(String),

    ServiceName(usize, String),
    PickRenderService(usize, usize),
    RenderServiceId(usize, String),
    AddService,
    RemoveService(usize),
    TestService(usize),
    ServiceTested(usize, Result<Vec<SeriesResult>, String>),

    MetricLabel(usize, usize, String),
    MetricExpr(usize, usize, String),
    MetricUnit(usize, usize, usize),
    MetricWorse(usize, usize, usize),
    MetricFloor(usize, usize, String),
    AddMetric(usize),
    RemoveMetric(usize, usize),

    Save,
    OpenUrl(String),
}

impl cosmic::Application for SettingsApp {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.orta.cosmic-deploy-sidebar.settings";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(mut core: cosmic::Core, _flags: ()) -> (Self, Task<cosmic::Action<Message>>) {
        core.set_header_title("Deploy Sidebar Settings".into());
        let (config, status) = match config::load_for_editing() {
            Ok(config) => (config, None),
            Err(e) => (
                toml::from_str(config::EXAMPLE).expect("example config parses"),
                Some(Err(format!("{e}. Showing the example config instead."))),
            ),
        };

        let has_grafana_token = keyring::grafana_token().is_some();
        let has_render_key = keyring::render_api_key().is_some();
        let mut app = SettingsApp {
            core,
            window_minutes: config.window_minutes.to_string(),
            baseline_minutes: config.baseline_minutes.to_string(),
            floors: floor_inputs(&config),
            config,
            grafana_token_input: String::new(),
            has_grafana_token,
            datasources: Load::NotLoaded,
            datasource_labels: Vec::new(),
            grafana_test: Load::NotLoaded,
            render_key_input: String::new(),
            has_render_key,
            render_services: Load::NotLoaded,
            render_service_labels: Vec::new(),
            service_tests: HashMap::new(),
            status,
        };

        // Fill the pickers straight away when the credentials are already there.
        let tasks = [app.load_datasources(), app.load_render_services()];
        (app, Task::batch(tasks))
    }

    fn view(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let mut column = widget::column::with_capacity(self.config.services.len() + 4)
            .spacing(space.space_l)
            .padding(space.space_l)
            .max_width(900);

        column = column
            .push(self.grafana_section())
            .push(self.render_section())
            .push(self.timing_section());
        for (index, service) in self.config.services.iter().enumerate() {
            column = column.push(self.service_section(index, service));
        }
        column = column.push(
            widget::button::standard("Add service")
                .leading_icon(widget::icon::from_name("list-add-symbolic"))
                .on_press(Message::AddService),
        );

        widget::scrollable(
            widget::container(column)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .into()
    }

    fn footer(&self) -> Option<Element<'_, Message>> {
        let space = cosmic::theme::spacing();
        let status: Element<'_, Message> = match &self.status {
            Some(Ok(message)) => widget::text::body(message.clone()).into(),
            Some(Err(message)) => widget::text::body(message.clone())
                .class(cosmic::theme::Text::Accent)
                .into(),
            None => widget::text::caption(
                "Saving updates the config file; a running sidebar reloads it within a few seconds.",
            )
            .into(),
        };
        Some(
            widget::row::with_capacity(3)
                .spacing(space.space_s)
                .padding(space.space_xs)
                .align_y(Alignment::Center)
                .push(widget::container(status).width(Length::Fill))
                .push(widget::button::suggested("Save").on_press(Message::Save))
                .into(),
        )
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::GrafanaUrl(url) => {
                self.config.grafana_url = url;
                self.grafana_test = Load::NotLoaded;
            }
            Message::GrafanaToken(token) => {
                self.grafana_token_input = token;
                self.grafana_test = Load::NotLoaded;
            }
            Message::LoadDatasources => return self.load_datasources(),
            Message::DatasourcesLoaded(result) => {
                self.datasources = match result {
                    Ok(datasources) => {
                        self.datasource_labels = datasources
                            .iter()
                            .map(|d| format!("{} ({})", d.name, d.uid))
                            .collect();
                        // With a single Prometheus there is nothing to choose.
                        if self.config.datasource_uid.is_empty() && datasources.len() == 1 {
                            self.config.datasource_uid.clone_from(&datasources[0].uid);
                        }
                        Load::Loaded(datasources)
                    }
                    Err(e) => Load::Failed(e),
                };
            }
            Message::PickDatasource(index) => {
                if let Load::Loaded(datasources) = &self.datasources
                    && let Some(datasource) = datasources.get(index)
                {
                    self.config.datasource_uid = datasource.uid.clone();
                    self.grafana_test = Load::NotLoaded;
                }
            }
            Message::DatasourceUid(uid) => {
                self.config.datasource_uid = uid;
                self.grafana_test = Load::NotLoaded;
            }
            Message::TestGrafana => {
                let Some(grafana) = self.grafana() else {
                    self.grafana_test = Load::Failed("Enter a URL and token first".into());
                    return Task::none();
                };
                self.grafana_test = Load::Loading;
                return task(async move { Message::GrafanaTested(grafana.test().await) });
            }
            Message::GrafanaTested(result) => {
                self.grafana_test = match result {
                    Ok(()) => Load::Loaded(()),
                    Err(e) => Load::Failed(e),
                };
            }

            Message::RenderKey(key) => self.render_key_input = key,
            Message::LoadRenderServices => return self.load_render_services(),
            Message::RenderServicesLoaded(result) => {
                self.render_services = match result {
                    Ok(services) => {
                        self.render_service_labels =
                            services.iter().map(|s| s.name.clone()).collect();
                        Load::Loaded(services)
                    }
                    Err(e) => Load::Failed(e),
                };
            }

            Message::WindowMinutes(text) => self.window_minutes = text,
            Message::BaselineMinutes(text) => self.baseline_minutes = text,

            Message::ServiceName(index, name) => {
                if let Some(service) = self.config.services.get_mut(index) {
                    service.name = name;
                }
            }
            Message::PickRenderService(index, choice) => {
                if let Load::Loaded(services) = &self.render_services
                    && let Some(picked) = services.get(choice)
                    && let Some(service) = self.config.services.get_mut(index)
                {
                    service.render_service_id.clone_from(&picked.id);
                    if service.name.is_empty() {
                        service.name.clone_from(&picked.name);
                    }
                }
            }
            Message::RenderServiceId(index, id) => {
                if let Some(service) = self.config.services.get_mut(index) {
                    service.render_service_id = id;
                }
            }
            Message::AddService => {
                self.config.services.push(Service {
                    name: String::new(),
                    render_service_id: String::new(),
                    metrics: Vec::new(),
                });
                self.floors.push(Vec::new());
            }
            Message::RemoveService(index) => {
                if index < self.config.services.len() {
                    self.config.services.remove(index);
                    self.floors.remove(index);
                    // Test results are keyed by position, which just shifted.
                    self.service_tests.clear();
                }
            }
            Message::TestService(index) => return self.test_service(index),
            Message::ServiceTested(index, result) => {
                let load = match result {
                    Ok(results) => Load::Loaded(results),
                    Err(e) => Load::Failed(e),
                };
                self.service_tests.insert(index, load);
            }

            Message::MetricLabel(s, m, label) => {
                if let Some(metric) = self.metric_mut(s, m) {
                    metric.label = label;
                }
            }
            Message::MetricExpr(s, m, expr) => {
                if let Some(metric) = self.metric_mut(s, m) {
                    metric.expr = expr;
                }
                self.service_tests.remove(&s);
            }
            Message::MetricUnit(s, m, choice) => {
                if let Some(metric) = self.metric_mut(s, m) {
                    metric.unit = UNITS[choice].0;
                }
            }
            Message::MetricWorse(s, m, choice) => {
                if let Some(metric) = self.metric_mut(s, m) {
                    metric.worse = DIRECTIONS[choice];
                }
            }
            Message::MetricFloor(s, m, text) => {
                if let Some(floor) = self.floors.get_mut(s).and_then(|f| f.get_mut(m)) {
                    *floor = text;
                }
            }
            Message::AddMetric(s) => {
                if let Some(service) = self.config.services.get_mut(s) {
                    service.metrics.push(Metric {
                        label: String::new(),
                        expr: String::new(),
                        unit: Unit::Count,
                        worse: Direction::Higher,
                        floor: 0.0,
                    });
                    self.floors[s].push("0".into());
                    self.service_tests.remove(&s);
                }
            }
            Message::RemoveMetric(s, m) => {
                if let Some(service) = self.config.services.get_mut(s)
                    && m < service.metrics.len()
                {
                    service.metrics.remove(m);
                    self.floors[s].remove(m);
                    self.service_tests.remove(&s);
                }
            }

            Message::Save => self.status = Some(self.save()),
            Message::OpenUrl(url) => {
                let _ = open::that_detached(url);
            }
        }
        Task::none()
    }
}

impl SettingsApp {
    fn grafana_section(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let url = self.config.grafana_url.trim_end_matches('/');

        let token_placeholder = if self.has_grafana_token {
            "Stored in the keyring, type to replace"
        } else {
            "glsa_…"
        };
        let token = widget::column::with_capacity(2)
            .spacing(space.space_xxs)
            .push(
                widget::text_input(token_placeholder, &self.grafana_token_input)
                    .on_input(Message::GrafanaToken)
                    .password(),
            )
            .push(widget::text::caption(
                "A service account token with the Viewer role. GitHub sign-in doesn't apply to tokens.",
            ));

        let token_help = (!url.is_empty()).then(|| {
            widget::button::link("Create one in Grafana")
                .on_press(Message::OpenUrl(format!("{url}/org/serviceaccounts")))
                .trailing_icon(true)
        });

        let selected = match &self.datasources {
            Load::Loaded(datasources) => datasources
                .iter()
                .position(|d| d.uid == self.config.datasource_uid),
            _ => None,
        };
        let datasource: Element<'_, Message> = match &self.datasources {
            Load::Loaded(datasources) if !datasources.is_empty() => widget::dropdown(
                self.datasource_labels.as_slice(),
                selected,
                Message::PickDatasource,
            )
            .into(),
            _ => widget::text_input("Datasource UID", &self.config.datasource_uid)
                .on_input(Message::DatasourceUid)
                .width(Length::Fixed(240.0))
                .into(),
        };
        let datasource_note = match &self.datasources {
            Load::Loading => Some("Loading datasources…".to_string()),
            Load::Loaded(d) if d.is_empty() => {
                Some("No Prometheus datasources visible to this token".to_string())
            }
            Load::Failed(e) => Some(e.clone()),
            Load::NotLoaded | Load::Loaded(_) => None,
        };

        let test_result: Element<'_, Message> = match &self.grafana_test {
            Load::NotLoaded => widget::Space::new(0, 0).into(),
            Load::Loading => widget::text::caption("Testing…").into(),
            Load::Loaded(()) => widget::text::caption("Connected, queries work").into(),
            Load::Failed(e) => widget::text::caption(e.clone())
                .class(cosmic::theme::Text::Accent)
                .into(),
        };

        widget::settings::section()
            .title("Grafana")
            .add(widget::settings::item(
                "URL",
                widget::text_input("https://grafana.example.com", &self.config.grafana_url)
                    .on_input(Message::GrafanaUrl)
                    .width(Length::Fixed(360.0)),
            ))
            .add(widget::settings::flex_item("Token", token))
            .add_maybe(token_help)
            .add(widget::settings::item(
                "Prometheus datasource",
                widget::row::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_y(Alignment::Center)
                    .push(datasource)
                    .push(
                        widget::button::icon(widget::icon::from_name("view-refresh-symbolic"))
                            .on_press(Message::LoadDatasources),
                    ),
            ))
            .add_maybe(datasource_note.map(widget::text::caption))
            .add(
                widget::row::with_capacity(2)
                    .spacing(space.space_s)
                    .align_y(Alignment::Center)
                    .push(
                        widget::button::standard("Test connection").on_press(Message::TestGrafana),
                    )
                    .push(test_result),
            )
            .into()
    }

    fn render_section(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let placeholder = if self.has_render_key {
            "Stored key in use, type to replace"
        } else {
            "rnd_…"
        };
        let note = match &self.render_services {
            Load::NotLoaded | Load::Loaded(_) => None,
            Load::Loading => Some("Loading services…".to_string()),
            Load::Failed(e) => Some(e.clone()),
        };

        widget::settings::section()
            .title("Render")
            .add(widget::settings::flex_item(
                "API key",
                widget::row::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_y(Alignment::Center)
                    .push(
                        widget::text_input(placeholder, &self.render_key_input)
                            .on_input(Message::RenderKey)
                            .password()
                            .width(Length::Fill),
                    )
                    .push(
                        widget::button::standard("Load services")
                            .on_press(Message::LoadRenderServices),
                    ),
            ))
            .add(
                widget::button::link("Create a Render API key")
                    .on_press(Message::OpenUrl(RENDER_API_KEYS_URL.into()))
                    .trailing_icon(true),
            )
            .add_maybe(note.map(widget::text::caption))
            .into()
    }

    fn timing_section(&self) -> Element<'_, Message> {
        widget::settings::section()
            .title("Timing")
            .add(widget::settings::item(
                "Minutes to show after a deploy goes live",
                widget::text_input("10", &self.window_minutes)
                    .on_input(Message::WindowMinutes)
                    .width(Length::Fixed(80.0)),
            ))
            .add(widget::settings::item(
                "Minutes before the deploy to compare against",
                widget::text_input("30", &self.baseline_minutes)
                    .on_input(Message::BaselineMinutes)
                    .width(Length::Fixed(80.0)),
            ))
            .into()
    }

    fn service_section<'a>(&'a self, index: usize, service: &'a Service) -> Element<'a, Message> {
        let space = cosmic::theme::spacing();

        let render_picker: Element<'_, Message> = match &self.render_services {
            Load::Loaded(services) => {
                let selected = services
                    .iter()
                    .position(|s| s.id == service.render_service_id);
                widget::dropdown(
                    self.render_service_labels.as_slice(),
                    selected,
                    move |choice| Message::PickRenderService(index, choice),
                )
                .into()
            }
            _ => widget::text_input("srv-…", &service.render_service_id)
                .on_input(move |id| Message::RenderServiceId(index, id))
                .width(Length::Fixed(260.0))
                .into(),
        };

        let tests = self.service_tests.get(&index);
        let mut section = widget::settings::section()
            .title(if service.name.is_empty() {
                "New service"
            } else {
                service.name.as_str()
            })
            .add(widget::settings::item(
                "Name",
                widget::text_input("api", &service.name)
                    .on_input(move |name| Message::ServiceName(index, name))
                    .width(Length::Fixed(260.0)),
            ))
            .add(widget::settings::item("Render service", render_picker));

        for (m, metric) in service.metrics.iter().enumerate() {
            let result = match tests {
                Some(Load::Loaded(results)) => Some(match results.get(m) {
                    Some(Ok(points)) => points.last().map_or_else(
                        || "No data".to_string(),
                        |(_, v)| format!("Now: {}", health::format(metric.unit, *v)),
                    ),
                    Some(Err(e)) => format!("Error: {e}"),
                    None => "No result".to_string(),
                }),
                _ => None,
            };
            section = section.add(self.metric_editor(index, m, metric, result));
        }

        let test_note: Option<Element<'_, Message>> = match tests {
            Some(Load::Loading) => Some(widget::text::caption("Running queries…").into()),
            Some(Load::Failed(e)) => Some(
                widget::text::caption(e.clone())
                    .class(cosmic::theme::Text::Accent)
                    .into(),
            ),
            _ => None,
        };

        section
            .add(
                widget::row::with_capacity(4)
                    .spacing(space.space_s)
                    .align_y(Alignment::Center)
                    .push(
                        widget::button::standard("Add metric")
                            .leading_icon(widget::icon::from_name("list-add-symbolic"))
                            .on_press(Message::AddMetric(index)),
                    )
                    .push(
                        widget::button::standard("Test queries")
                            .on_press(Message::TestService(index)),
                    )
                    .push(widget::horizontal_space())
                    .push(
                        widget::button::destructive("Remove service")
                            .on_press(Message::RemoveService(index)),
                    ),
            )
            .add_maybe(test_note)
            .into()
    }

    fn metric_editor<'a>(
        &'a self,
        s: usize,
        m: usize,
        metric: &'a Metric,
        result: Option<String>,
    ) -> Element<'a, Message> {
        let space = cosmic::theme::spacing();
        let unit = UNITS.iter().position(|(u, _)| *u == metric.unit);
        let worse = DIRECTIONS.iter().position(|d| *d == metric.worse);
        let floor = self
            .floors
            .get(s)
            .and_then(|f| f.get(m))
            .map_or("", String::as_str);

        let top = widget::row::with_capacity(6)
            .spacing(space.space_xs)
            .align_y(Alignment::Center)
            .push(
                widget::text_input("Label", &metric.label)
                    .on_input(move |label| Message::MetricLabel(s, m, label))
                    .width(Length::Fill),
            )
            .push(widget::dropdown(&UNIT_LABELS[..], unit, move |choice| {
                Message::MetricUnit(s, m, choice)
            }))
            .push(widget::dropdown(
                &DIRECTION_LABELS[..],
                worse,
                move |choice| Message::MetricWorse(s, m, choice),
            ))
            .push(widget::text::caption("Floor"))
            .push(
                widget::text_input("0", floor)
                    .on_input(move |text| Message::MetricFloor(s, m, text))
                    .width(Length::Fixed(90.0)),
            )
            .push(
                widget::button::icon(widget::icon::from_name("edit-delete-symbolic"))
                    .on_press(Message::RemoveMetric(s, m)),
            );

        let mut column = widget::column::with_capacity(3)
            .spacing(space.space_xxs)
            .push(top)
            .push(
                widget::text_input("PromQL, e.g. sum(rate(…[2m]))", &metric.expr)
                    .on_input(move |expr| Message::MetricExpr(s, m, expr)),
            );
        if let Some(result) = result {
            column = column.push(widget::text::caption(result));
        }
        column.into()
    }

    fn metric_mut(&mut self, s: usize, m: usize) -> Option<&mut Metric> {
        self.config.services.get_mut(s)?.metrics.get_mut(m)
    }

    /// A client from what is on screen: the typed token, else the stored one.
    fn grafana(&self) -> Option<Grafana> {
        let url = self.config.grafana_url.trim();
        if url.is_empty() {
            return None;
        }
        let token = Some(self.grafana_token_input.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(keyring::grafana_token)?;
        Some(Grafana {
            url: url.to_string(),
            token,
            datasource_uid: self.config.datasource_uid.trim().to_string(),
        })
    }

    fn render_key(&self) -> Option<String> {
        Some(self.render_key_input.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(keyring::render_api_key)
    }

    fn load_datasources(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(grafana) = self.grafana() else {
            return Task::none();
        };
        self.datasources = Load::Loading;
        task(async move { Message::DatasourcesLoaded(grafana.prometheus_datasources().await) })
    }

    fn load_render_services(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(key) = self.render_key() else {
            self.render_services = Load::NotLoaded;
            return Task::none();
        };
        self.render_services = Load::Loading;
        task(async move { Message::RenderServicesLoaded(render::services(&key).await) })
    }

    /// Run a service's queries over the last 15 minutes and show each one's latest value.
    fn test_service(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        let Some(grafana) = self.grafana() else {
            self.service_tests
                .insert(index, Load::Failed("Set up Grafana above first".into()));
            return Task::none();
        };
        let Some(service) = self.config.services.get(index) else {
            return Task::none();
        };
        let exprs: Vec<String> = service.metrics.iter().map(|m| m.expr.clone()).collect();
        self.service_tests.insert(index, Load::Loading);
        task(async move {
            let to = Utc::now();
            let result = grafana
                .query_range(
                    &exprs,
                    to - TimeDelta::minutes(15),
                    to,
                    Duration::from_mins(1),
                )
                .await;
            Message::ServiceTested(index, result)
        })
    }

    /// Parse the number fields, store any new credentials, and write the config.
    fn save(&mut self) -> Result<String, String> {
        let mut config = self.config.clone();
        config.grafana_url = config.grafana_url.trim().trim_end_matches('/').to_string();
        config.window_minutes = parse_minutes(&self.window_minutes, "Minutes to show")?;
        config.baseline_minutes = parse_minutes(&self.baseline_minutes, "Baseline minutes")?;
        for (s, service) in config.services.iter_mut().enumerate() {
            for (m, metric) in service.metrics.iter_mut().enumerate() {
                let text = self.floors[s][m].trim();
                metric.floor = if text.is_empty() {
                    0.0
                } else {
                    text.parse()
                        .map_err(|_| format!("The floor for {} isn't a number", metric.label))?
                };
            }
        }

        let grafana_token = self.grafana_token_input.trim();
        if !grafana_token.is_empty() {
            keyring::store_grafana_token(grafana_token).map_err(|e| e.to_string())?;
            self.grafana_token_input.clear();
            self.has_grafana_token = true;
        }
        let render_key = self.render_key_input.trim();
        if !render_key.is_empty() {
            keyring::store_render_api_key(render_key).map_err(|e| e.to_string())?;
            self.render_key_input.clear();
            self.has_render_key = true;
        }

        config::save(&config)?;
        self.config = config;
        Ok(match self.config.validate() {
            Ok(()) => "Saved".to_string(),
            Err(missing) => format!("Saved, but the sidebar won't run yet: {missing}"),
        })
    }
}

fn floor_inputs(config: &Config) -> Vec<Vec<String>> {
    config
        .services
        .iter()
        .map(|s| s.metrics.iter().map(|m| m.floor.to_string()).collect())
        .collect()
}

fn parse_minutes(text: &str, field: &str) -> Result<i64, String> {
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{field} must be a whole number above zero"))
}

/// Run a future and deliver its output as an app message.
fn task(future: impl Future<Output = Message> + Send + 'static) -> Task<cosmic::Action<Message>> {
    cosmic::task::future(future).map(cosmic::Action::App)
}
