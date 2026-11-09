// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Foundation
import Testing

/// A model with the example config and no credentials, so nothing touches the network.
@MainActor
private func model() throws -> SidebarModel {
    SidebarModel(setup: Setup(config: try Config.parse(exampleConfig)))
}

private func deploy(_ id: String, _ status: DeployStatus, finishedAt: Date? = nil) -> Deploy {
    Deploy(
        id: id, status: status, revision: "abc1234", commitMessage: "Change", createdAt: Date(),
        finishedAt: finishedAt, url: "")
}

@MainActor @Test func startupIgnoresDeploysThatAlreadyFinished() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .live))
    #expect(model.watches.isEmpty)
    #expect(!model.isShowing)
}

@MainActor @Test func startupShowsADeployUnderWay() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .building))
    #expect(model.watches.map(\.phase) == [.building])
}

@MainActor @Test func newDeployIsShownAndGoesLiveKeepingBaselines() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .live))
    model.deployFetched(service: 0, deploy: deploy("d2", .building))
    #expect(model.watches.map(\.id) == ["d2"])

    let baselines: [SeriesResult] = model.metrics(0).map { _ in .success([Point(time: Date(), value: 2)]) }
    model.metricsFetched(deployID: "d2", baseline: true, result: .success(baselines))

    let liveAt = Date(timeIntervalSince1970: 1000)
    model.deployFetched(service: 0, deploy: deploy("d2", .live, finishedAt: liveAt))
    #expect(model.watches.map(\.phase) == [.live(liveAt)])
    #expect(model.watches[0].baselines.allSatisfy { $0 == 2 })
}

@MainActor @Test func deployThatWentLiveBetweenPollsIsShownLive() throws {
    let model = try model()
    model.deployFetched(service: 1, deploy: deploy("d1", .live))
    let liveAt = Date(timeIntervalSince1970: 2000)
    model.deployFetched(service: 1, deploy: deploy("d2", .live, finishedAt: liveAt))
    #expect(model.watches.map(\.phase) == [.live(liveAt)])
    #expect(model.watches[0].service == 1)
}

@MainActor @Test func canceledDeployIsRemoved() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .building))
    model.deployFetched(service: 0, deploy: deploy("d1", .canceled))
    #expect(model.watches.isEmpty)
}

@MainActor @Test func failedBuildIsShown() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .building))
    model.deployFetched(service: 0, deploy: deploy("d1", .failed))
    #expect(model.watches.map(\.phase) == [.failed])
    #expect(model.status(model.watches[0]) == WatchStatus(text: "deploy failed", tone: .level(.bad)))
}

@MainActor @Test func previewShowsTheLatestDeployAsLive() throws {
    let model = try model()
    model.previewLatest("puzzmo.com")
    model.deployFetched(service: 1, deploy: deploy("d1", .live))
    #expect(model.watches.count == 1)
    guard case .live = model.watches[0].phase else {
        Issue.record("expected a live watch")
        return
    }
}

@MainActor @Test func worstLevelFollowsTheMetrics() throws {
    let model = try model()
    model.deployFetched(service: 0, deploy: deploy("d1", .building))
    model.deployFetched(service: 0, deploy: deploy("d1", .live, finishedAt: Date()))
    let metrics = model.metrics(0)
    model.metricsFetched(
        deployID: "d1", baseline: true,
        result: .success(metrics.map { _ in .success([Point(time: Date(), value: 1)]) }))
    // p95 latency (index 1) triples; everything else holds steady.
    model.metricsFetched(
        deployID: "d1", baseline: false,
        result: .success(metrics.indices.map { .success([Point(time: Date(), value: $0 == 1 ? 3 : 1)]) }))
    #expect(model.worstLevel(model.watches[0]) == .bad)
}

/// A live api watch with every metric at its baseline, or with p95 latency tripled.
@MainActor
private func liveModel(liveAt: Date, regressed: Bool) throws -> SidebarModel {
    let model = try model()
    model.clock = { liveAt }
    model.deployFetched(service: 0, deploy: deploy("d1", .building))
    model.deployFetched(service: 0, deploy: deploy("d1", .live, finishedAt: liveAt))
    let metrics = model.metrics(0)
    model.metricsFetched(
        deployID: "d1", baseline: true,
        result: .success(metrics.map { _ in .success([Point(time: liveAt, value: 1)]) }))
    model.metricsFetched(
        deployID: "d1", baseline: false,
        result: .success(metrics.indices.map {
            .success([Point(time: liveAt, value: regressed && $0 == 1 ? 3 : 1)])
        }))
    return model
}

@MainActor @Test func healthyDeployFadesOutAfterTheWindow() throws {
    let liveAt = Date(timeIntervalSince1970: 10_000)
    let model = try liveModel(liveAt: liveAt, regressed: false)

    model.clock = { liveAt.addingTimeInterval(9 * 60) }
    model.tick()
    #expect(model.fadeStarted == nil)
    #expect(model.status(model.watches[0]).text == "live · 1m 00s left")

    let windowEnd = liveAt.addingTimeInterval(10 * 60)
    model.clock = { windowEnd }
    model.tick()
    #expect(model.fadeStarted == windowEnd)
    #expect(model.isShowing)

    model.clock = { windowEnd.addingTimeInterval(SidebarModel.fadeOut) }
    model.tick()
    #expect(model.watches.isEmpty)
    #expect(model.fadeStarted == nil)
    #expect(!model.isShowing)
}

@MainActor @Test func badDeployIsHeldPastTheWindow() throws {
    let liveAt = Date(timeIntervalSince1970: 10_000)
    let model = try liveModel(liveAt: liveAt, regressed: true)
    model.clock = { liveAt.addingTimeInterval(30 * 60) }
    model.tick()
    #expect(model.fadeStarted == nil)
    #expect(model.watches.count == 1)
    #expect(model.status(model.watches[0]) == WatchStatus(text: "live · held, something looks off", tone: .level(.bad)))
}

@MainActor @Test func newDeployCancelsTheFade() throws {
    let liveAt = Date(timeIntervalSince1970: 10_000)
    let model = try liveModel(liveAt: liveAt, regressed: false)
    model.clock = { liveAt.addingTimeInterval(10 * 60) }
    model.tick()
    #expect(model.fadeStarted != nil)

    model.deployFetched(service: 1, deploy: deploy("d2", .building))
    model.deployFetched(service: 1, deploy: deploy("d3", .building))
    #expect(model.fadeStarted == nil)
    model.tick()
    // The finished api watch goes; the building one stays.
    #expect(model.watches.map(\.id) == ["d3"])
}
