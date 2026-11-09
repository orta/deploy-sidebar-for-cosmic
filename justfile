name := 'cosmic-deploy-sidebar'
bin-dst := env('HOME') / '.local' / 'bin' / name
unit-dst := env('HOME') / '.config' / 'systemd' / 'user' / name + '.service'
desktop-dst := env('HOME') / '.local' / 'share' / 'applications' / name + '-settings.desktop'

default: build-release

build-release *args:
    cargo build --release {{args}}

check:
    cargo clippy -- -W clippy::pedantic

test:
    cargo test

# Open the settings window
settings:
    cargo run -- settings

# Show the sidebar with synthetic data
demo:
    cargo run -- --demo

# Install the binary and a systemd user service, then (re)start it
install: build-release
    install -Dm0755 target/release/{{name}} {{bin-dst}}
    install -Dm0644 resources/{{name}}.service {{unit-dst}}
    install -Dm0644 resources/{{name}}-settings.desktop {{desktop-dst}}
    systemctl --user daemon-reload
    systemctl --user enable --now {{name}}.service
    systemctl --user restart {{name}}.service

uninstall:
    -systemctl --user disable --now {{name}}.service
    rm -f {{bin-dst}} {{unit-dst}} {{desktop-dst}}

logs:
    journalctl --user -u {{name}} -f
