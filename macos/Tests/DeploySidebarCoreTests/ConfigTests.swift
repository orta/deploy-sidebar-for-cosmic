// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Foundation
import Testing

@Test func exampleConfigParses() throws {
    let config = try Config.parse(exampleConfig)
    #expect(config.windowMinutes == 10)
    #expect(config.services.count == 2)
    #expect(config.services.allSatisfy { !$0.metrics.isEmpty })
    #expect(config.services[0].metrics[1].unit == .seconds)
    #expect(config.services[0].metrics[1].floor == 0.2)
    // Whole-number floors are written as integers.
    #expect(config.services[1].metrics[3].floor == 5)
    #expect(config.services[0].metrics[0].expr == #"sum(rate(http_request_duration_seconds_count{job="puzzmo-api"}[2m]))"#)
}

@Test func embeddedExampleMatchesTheRepo() throws {
    let repo = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let onDisk = try String(contentsOf: repo.appendingPathComponent("config.example.toml"), encoding: .utf8)
    #expect(onDisk == exampleConfig, "Run `make example` after editing config.example.toml")
}

@Test func configRoundTripsThroughTOML() throws {
    var config = try Config.parse(exampleConfig)
    config.services[0].name = "it's \"quoted\""
    config.services[1].metrics[0].floor = 0.000_01
    let text = config.toTOML()
    #expect(try Config.parse(text) == config)
}

@Test func missingFieldsAreNamed() {
    #expect(throws: AppError("missing field `datasource_uid`")) {
        try Config.parse(#"grafana_url = "x""#)
    }
    let badUnit = """
        grafana_url = "x"
        datasource_uid = "y"
        [[services]]
        name = "api"
        render_service_id = "srv"
        [[services.metrics]]
        label = "a"
        expr = "b"
        unit = "parsecs"
        worse = "higher"
        """
    #expect(throws: AppError(
        "services[0].metrics[0]: unknown unit \"parsecs\", expected one of count, per_second, seconds, bytes, ratio"
    )) {
        try Config.parse(badUnit)
    }
}

@Test func validation() throws {
    var config = try Config.parse(exampleConfig)
    #expect(config.validationProblem() == "Pick a Prometheus datasource in settings")
    config.grafanaURL = " "
    #expect(config.validationProblem() == "Set the Grafana URL in settings")
    config.grafanaURL = "https://grafana.example.com"
    config.datasourceUID = "abc"
    #expect(config.validationProblem() == "Pick a Render service for puzzmo.com in settings")
    config.services[1].renderServiceID = "srv-2"
    #expect(config.validationProblem() == nil)
}
