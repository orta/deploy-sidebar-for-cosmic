// SPDX-License-Identifier: GPL-3.0

import Foundation

public struct Point: Sendable, Equatable {
    public var time: Date
    public var value: Double

    /// Make a point.
    public init(time: Date, value: Double) {
        self.time = time
        self.value = value
    }
}

/// One query's result: points sorted by time, or why it failed.
public typealias SeriesResult = Result<[Point], AppError>

/// A Prometheus datasource the token can see.
public struct Datasource: Sendable, Equatable, Identifiable {
    public let name: String
    public let uid: String
    public var id: String { uid }
}

/// Runs Prometheus queries through Grafana's `/api/ds/query`, so Prometheus never needs to be reachable.
public struct Grafana: Sendable {
    public var url: String
    public var token: String
    public var datasourceUID: String

    /// Make a client.
    public init(url: String, token: String, datasourceUID: String) {
        self.url = url
        self.token = token
        self.datasourceUID = datasourceUID
    }

    static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 20
        return URLSession(configuration: configuration)
    }()

    /// Run every expression over `from...to`, returning one result per expression in order.
    public func queryRange(_ exprs: [String], from: Date, to: Date, step: TimeInterval) async throws -> [SeriesResult] {
        let stepMs = Int(step * 1000)
        let queries: [[String: Any]] = exprs.enumerated().map { index, expr in
            [
                "refId": Self.refID(index),
                "datasource": ["uid": datasourceUID, "type": "prometheus"],
                "expr": expr,
                "range": true,
                "instant": false,
                "intervalMs": stepMs,
                "maxDataPoints": 500,
            ]
        }
        let body: [String: Any] = [
            "queries": queries,
            "from": String(Int64(from.timeIntervalSince1970 * 1000)),
            "to": String(Int64(to.timeIntervalSince1970 * 1000)),
        ]

        var request = try request(path: "/api/ds/query")
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        // A failing query still answers with per-refId errors, so only bail on auth problems.
        let data = try await send(request)
        return try Self.parseResults(data, count: exprs.count)
    }

    /// List Prometheus datasources. Uses the frontend settings, which a Viewer token can read,
    /// unlike `/api/datasources`.
    public func prometheusDatasources() async throws -> [Datasource] {
        try Self.parseDatasources(try await send(try request(path: "/api/frontend/settings")))
    }

    /// Run a trivial query, to confirm the URL, token and datasource work together.
    public func test() async throws {
        let now = Date()
        let results = try await queryRange(["vector(1)"], from: now.addingTimeInterval(-300), to: now, step: 60)
        switch results.first {
        case .success(let points)?:
            if points.isEmpty { throw AppError("The datasource answered but returned no data") }
        case .failure(let error)?:
            throw error
        case nil:
            throw AppError("No result")
        }
    }

    private func request(path: String) throws -> URLRequest {
        let base = url.hasSuffix("/") ? String(url.dropLast()) : url
        guard let url = URL(string: base + path), url.scheme != nil else {
            throw AppError("\(self.url) isn't a valid URL")
        }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        return request
    }

    /// Send a request, explaining the usual causes when Grafana refuses or answers with a web page.
    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await Self.session.data(for: request)
        } catch {
            throw AppError(error.localizedDescription)
        }
        let http = response as? HTTPURLResponse
        let status = http?.statusCode ?? 0
        if status == 401 || status == 403 {
            throw AppError("Grafana rejected the token (HTTP \(status))")
        }
        if let type = http?.value(forHTTPHeaderField: "Content-Type"), type.contains("text/html") {
            let from = response.url?.absoluteString ?? request.url?.absoluteString ?? "Grafana"
            throw AppError(
                "Got a web page instead of JSON from \(from). A login proxy in front of Grafana would cause this.")
        }
        guard !data.isEmpty || (200..<300).contains(status) else {
            throw AppError("Unexpected Grafana response (HTTP \(status))")
        }
        return data
    }

    private static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw AppError("Unexpected Grafana response: \(AppError.describe(error))")
        }
    }

    /// The Prometheus datasources in `/api/frontend/settings`. Built-in entries such as
    /// `-- Dashboard --` have no uid, so every field is optional.
    static func parseDatasources(_ data: Data) throws -> [Datasource] {
        struct FrontendDatasource: Decodable {
            let uid: String?
            let type: String?
        }
        struct FrontendSettings: Decodable {
            let datasources: [String: FrontendDatasource]?
        }
        return (try decode(FrontendSettings.self, from: data).datasources ?? [:])
            .compactMap { name, datasource in
                guard datasource.type == "prometheus", let uid = datasource.uid, !uid.isEmpty else { return nil }
                return Datasource(name: name, uid: uid)
            }
            .sorted { $0.name < $1.name }
    }

    static func refID(_ index: Int) -> String {
        "Q\(index)"
    }

    /// Split a `/api/ds/query` response into one result per query, in refId order.
    static func parseResults(_ data: Data, count: Int) throws -> [SeriesResult] {
        let response = try decode(QueryResponse.self, from: data)
        // A request Grafana rejects as a whole, e.g. an unknown datasource, has only a message.
        if response.results == nil, let message = response.message {
            throw AppError("Grafana: \(message)")
        }
        return (0..<count).map { index in
            guard let result = response.results?[refID(index)] else {
                return .failure(AppError("No result"))
            }
            if let error = result.error {
                return .failure(AppError(error))
            }
            return .success(mergeFrames(result.frames ?? []))
        }
    }

    /// Sum every frame's values per timestamp, so a query returning several series still plots as one.
    static func mergeFrames(_ frames: [Frame]) -> [Point] {
        var points: [Int64: Double] = [:]
        for frame in frames {
            guard let columns = frame.data?.values, columns.count >= 2 else { continue }
            for (time, value) in zip(columns[0], columns[1]) {
                guard let time = time.number, let value = value.number, value.isFinite else { continue }
                points[Int64(time), default: 0] += value
            }
        }
        return points.keys.sorted().map { ms in
            Point(time: Date(timeIntervalSince1970: Double(ms) / 1000), value: points[ms] ?? 0)
        }
    }

    struct QueryResponse: Decodable {
        let results: [String: QueryResult]?
        let message: String?
    }

    struct QueryResult: Decodable {
        let frames: [Frame]?
        let error: String?
    }

    struct Frame: Decodable {
        let data: FrameData?
    }

    struct FrameData: Decodable {
        let values: [[JSONNumber]]?
    }

    /// A data frame cell: a number, or anything else (null, a string), which is skipped.
    struct JSONNumber: Decodable {
        let number: Double?

        init(from decoder: Decoder) throws {
            number = try? decoder.singleValueContainer().decode(Double.self)
        }
    }
}
