# SPDX-License-Identifier: GPL-3.0
#
# The macOS app. `make install` builds it, copies it to ~/Applications and starts it.

APP_NAME := Deploy Sidebar
BUNDLE_ID := io.orta.DeploySidebar
BUNDLE := build/$(APP_NAME).app
BINARY := $(BUNDLE)/Contents/MacOS/DeploySidebar
INSTALL_DIR ?= $(HOME)/Applications
# Ad-hoc by default. A stable identity, e.g. SIGN_IDENTITY="Apple Development: …", stops the
# Keychain asking again after every rebuild.
SIGN_IDENTITY ?= -

.PHONY: build test app run demo settings queries install uninstall logs example clean

build:
	swift build

test:
	swift test

# Assemble a signed .app bundle from a release build
app:
	swift build -c release
	rm -rf "$(BUNDLE)"
	mkdir -p "$(BUNDLE)/Contents/MacOS" "$(BUNDLE)/Contents/Resources"
	cp Info.plist "$(BUNDLE)/Contents/Info.plist"
	cp .build/release/DeploySidebar "$(BINARY)"
	codesign --force --sign "$(SIGN_IDENTITY)" "$(BUNDLE)"

run: app
	open "$(BUNDLE)"

# Synthetic deploys, no network. Quit it from its menu bar icon.
demo: app
	"$(BINARY)" --demo

# Only the settings window; a running sidebar picks up the saved config
settings: app
	"$(BINARY)" settings

# Run every configured query and print the results
queries: app
	"$(BINARY)" check

install: app
	-osascript -e 'quit app id "$(BUNDLE_ID)"' 2>/dev/null
	mkdir -p "$(INSTALL_DIR)"
	rm -rf "$(INSTALL_DIR)/$(APP_NAME).app"
	cp -R "$(BUNDLE)" "$(INSTALL_DIR)/"
	open "$(INSTALL_DIR)/$(APP_NAME).app"

uninstall:
	-osascript -e 'quit app id "$(BUNDLE_ID)"' 2>/dev/null
	rm -rf "$(INSTALL_DIR)/$(APP_NAME).app"

logs:
	log stream --style compact --predicate 'subsystem == "$(BUNDLE_ID)"'

# Re-embed ../config.example.toml after editing it; ConfigTests fails until this is run
example:
	@{ \
	  echo '// SPDX-License-Identifier: GPL-3.0'; \
	  echo '// Generated from ../config.example.toml by `make example`. ConfigTests checks they match.'; \
	  echo ''; \
	  echo '/// The example config, used by `init-config`, `--demo`, and settings when there is no config yet.'; \
	  echo 'public let exampleConfig = #"""'; \
	  cat ../config.example.toml; \
	  echo ''; \
	  echo '"""#'; \
	} > Sources/DeploySidebarCore/ExampleConfig.swift

clean:
	rm -rf .build build
