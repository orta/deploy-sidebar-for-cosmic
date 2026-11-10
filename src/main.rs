// SPDX-License-Identifier: GPL-3.0

mod app;
mod checklist;
mod cli;
mod config;
mod grafana;
mod health;
mod keyring;
mod placement;
mod render;
mod settings;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut demo = false;
    let preview = match args.first().map(String::as_str) {
        None => None,
        Some("--demo") => {
            demo = true;
            None
        }
        Some("--preview") => {
            let Some(service) = args.get(1) else {
                eprintln!("Usage: cosmic-deploy-sidebar --preview <service name>");
                std::process::exit(2);
            };
            Some(service.clone())
        }
        Some("settings") => {
            let settings = cosmic::app::Settings::default()
                .size(cosmic::iced::Size::new(860.0, 900.0))
                .size_limits(
                    cosmic::iced::Limits::NONE
                        .min_width(560.0)
                        .min_height(400.0),
                );
            if let Err(e) = cosmic::app::run::<settings::SettingsApp>(settings, ()) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            return;
        }
        Some(command) => std::process::exit(cli::run(command, &args[1..])),
    };

    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false);
    if let Err(e) = cosmic::app::run::<app::App>(settings, app::Flags { preview, demo }) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
