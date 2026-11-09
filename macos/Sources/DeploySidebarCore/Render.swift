// SPDX-License-Identifier: GPL-3.0

import Foundation

public enum DeployStatus: Sendable, Equatable {
    case building
    case live
    case failed
    case canceled
    /// A past deploy which a newer one replaced.
    case superseded
}

public struct Deploy: Sendable, Equatable {
    public var id: String
    public var status: DeployStatus
    /// Short commit SHA.
    public var revision: String?
    public var commitMessage: String?
    public var createdAt: Date
    public var finishedAt: Date?
    public var url: String

    /// Make a deploy.
    public init(
        id: String, status: DeployStatus, revision: String?, commitMessage: String?, createdAt: Date,
        finishedAt: Date?, url: String
    ) {
        self.id = id
        self.status = status
        self.revision = revision
        self.commitMessage = commitMessage
        self.createdAt = createdAt
        self.finishedAt = finishedAt
        self.url = url
    }

    /// A one-line summary of the commit, preferring the PR title for merge commits.
    public var commitSummary: String? {
        guard let commitMessage else { return nil }
        let lines = commitMessage.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        guard let first = lines.first else { return nil }
        if first.hasPrefix("Merge pull request") {
            return lines.count > 1 ? lines[1] : first
        }
        return first
    }
}

/// A service on the Render account, for the settings picker and `list-services`.
public struct RenderService: Sendable, Equatable, Identifiable, Decodable {
    public let id: String
    public let name: String
}

/// Render API client: just enough to notice deploys.
public enum Render {
    static let apiBase = "https://api.render.com/v1"

    static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 15
        return URLSession(configuration: configuration)
    }()

    /// Fetch the newest deploy of a service.
    public static func latestDeploy(apiKey: String, serviceID: String) async throws -> Deploy? {
        let data = try await get(apiKey: apiKey, path: "/services/\(serviceID)/deploys?limit=1")
        return try parseLatestDeploy(data, serviceID: serviceID)
    }

    /// Fetch the services on the account (first page only), sorted by name.
    public static func services(apiKey: String) async throws -> [RenderService] {
        struct Item: Decodable { let service: RenderService }
        let data = try await get(apiKey: apiKey, path: "/services?limit=100&suspended=not_suspended")
        let items = try decode([Item].self, from: data)
        return items.map(\.service).sorted { $0.name.lowercased() < $1.name.lowercased() }
    }

    static func parseLatestDeploy(_ data: Data, serviceID: String) throws -> Deploy? {
        struct Commit: Decodable {
            let id: String?
            let message: String?
        }
        struct RenderDeploy: Decodable {
            let id: String
            let status: String
            let createdAt: Date
            let finishedAt: Date?
            let commit: Commit?
        }
        struct Item: Decodable { let deploy: RenderDeploy }

        guard let deploy = try decode([Item].self, from: data).first?.deploy else { return nil }
        return Deploy(
            id: deploy.id,
            status: parseStatus(deploy.status),
            revision: deploy.commit?.id.map { String($0.prefix(7)) },
            commitMessage: deploy.commit?.message,
            createdAt: deploy.createdAt,
            finishedAt: deploy.finishedAt,
            url: "https://dashboard.render.com/web/\(serviceID)/deploys/\(deploy.id)"
        )
    }

    static func parseStatus(_ status: String) -> DeployStatus {
        switch status {
        case "created", "queued", "build_in_progress", "update_in_progress", "pre_deploy_in_progress":
            .building
        case "build_failed", "update_failed", "pre_deploy_failed":
            .failed
        case "canceled":
            .canceled
        case "deactivated":
            .superseded
        default:
            .live
        }
    }

    private static func get(apiKey: String, path: String) async throws -> Data {
        guard let url = URL(string: apiBase + path) else { throw AppError("Bad Render URL \(path)") }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw AppError(error.localizedDescription)
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 401 {
            throw AppError("Render rejected the API key")
        }
        guard (200..<300).contains(status) else {
            throw AppError("Render answered HTTP \(status) for \(url.absoluteString)")
        }
        return data
    }

    private static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let date = parseDate(text) else {
                throw DecodingError.dataCorrupted(
                    .init(codingPath: decoder.codingPath, debugDescription: "Not an RFC 3339 date: \(text)"))
            }
            return date
        }
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw AppError("Unexpected Render response: \(AppError.describe(error))")
        }
    }

    /// An RFC 3339 timestamp, with or without fractional seconds (Render sends microseconds).
    static func parseDate(_ text: String) -> Date? {
        (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(text))
            ?? (try? Date.ISO8601FormatStyle().parse(text))
    }
}
