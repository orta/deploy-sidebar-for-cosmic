// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Foundation
import Testing

@Test func renderStatuses() {
    #expect(Render.parseStatus("live") == .live)
    #expect(Render.parseStatus("deactivated") == .superseded)
    #expect(Render.parseStatus("update_in_progress") == .building)
    #expect(Render.parseStatus("build_failed") == .failed)
    #expect(Render.parseStatus("canceled") == .canceled)
}

@Test func renderDeployParses() throws {
    let json = """
        [{"deploy": {
          "id": "dep-abc", "status": "build_in_progress",
          "createdAt": "2025-05-19T09:26:43.123456Z", "finishedAt": null,
          "commit": {"id": "23675c8a1b2c3d", "message": "Merge pull request #5871 from x\\n\\nCalendar playlist\\n"}
        }, "cursor": "x"}]
        """
    let deploy = try #require(try Render.parseLatestDeploy(Data(json.utf8), serviceID: "srv-1"))
    #expect(deploy.id == "dep-abc")
    #expect(deploy.status == .building)
    #expect(deploy.revision == "23675c8")
    #expect(deploy.commitSummary == "Calendar playlist")
    #expect(deploy.finishedAt == nil)
    #expect(deploy.url == "https://dashboard.render.com/web/srv-1/deploys/dep-abc")
    #expect(deploy.createdAt.timeIntervalSince1970.rounded() == 1_747_646_803)

    #expect(try Render.parseLatestDeploy(Data("[]".utf8), serviceID: "srv-1") == nil)
}

@Test func renderDatesWithoutFractionalSeconds() {
    #expect(Render.parseDate("2025-05-19T09:26:43Z") != nil)
    #expect(Render.parseDate("2025-05-19T09:26:43.1+01:00") != nil)
    #expect(Render.parseDate("yesterday") == nil)
}

@Test func plainCommitSummary() {
    let deploy = Deploy(
        id: "d", status: .live, revision: nil, commitMessage: "  Tidy SSR logging  \nmore detail", createdAt: Date(),
        finishedAt: nil, url: "")
    #expect(deploy.commitSummary == "Tidy SSR logging")
}

@Test func framesAreSummedAndNullsSkipped() throws {
    let json = """
        {"results": {
          "Q0": {"status": 200, "frames": [
            {"schema": {}, "data": {"values": [[1000, 2000, 3000], [1.0, null, 3.0]]}},
            {"schema": {}, "data": {"values": [[1000, 2000], [0.5, 2.0]]}}
          ]},
          "Q1": {"status": 400, "error": "bad expr", "frames": []}
        }}
        """
    let results = try Grafana.parseResults(Data(json.utf8), count: 3)
    #expect(try results[0].get().map(\.value) == [1.5, 2.0, 3.0])
    #expect(try results[0].get().map(\.time.timeIntervalSince1970) == [1, 2, 3])
    #expect(results[1] == .failure(AppError("bad expr")))
    #expect(results[2] == .failure(AppError("No result")))
}

@Test func builtInDatasourcesWithoutUIDsAreSkipped() throws {
    let json = """
        {"datasources": {
          "-- Dashboard --": {"type": "datasource", "name": "-- Dashboard --", "meta": {}},
          "-- Grafana --": {"type": "datasource", "uid": "grafana"},
          "Prometheus": {"type": "prometheus", "uid": "abc123"},
          "Loki": {"type": "loki", "uid": "loki1"}
        }, "buildInfo": {}}
        """
    #expect(try Grafana.parseDatasources(Data(json.utf8)) == [Datasource(name: "Prometheus", uid: "abc123")])
}

@Test func wholeRequestErrorsAreShown() {
    let json = #"{"message": "Query missing datasource info: Q0", "messageId": "", "statusCode": 400}"#
    #expect(throws: AppError("Grafana: Query missing datasource info: Q0")) {
        try Grafana.parseResults(Data(json.utf8), count: 1)
    }
}

@Test func decodingErrorsAreReadable() {
    struct Needs: Decodable { let uid: String }
    #expect(throws: AppError("Unexpected Render response: `id` is missing at 0.deploy")) {
        _ = try Render.parseLatestDeploy(Data(#"[{"deploy": {}}]"#.utf8), serviceID: "s")
    }
    do {
        _ = try JSONDecoder().decode([String: Needs].self, from: Data(#"{"-- Dashboard --": {}}"#.utf8))
    } catch {
        #expect(AppError.describe(error) == "`uid` is missing at -- Dashboard --")
    }
}
