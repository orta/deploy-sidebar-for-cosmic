// SPDX-License-Identifier: GPL-3.0

import Foundation

/// A parsed TOML value. An array of tables is an `.array` of `.table`s.
public indirect enum TOMLValue: Equatable, Sendable {
    case string(String)
    case integer(Int)
    case float(Double)
    case bool(Bool)
    case array([TOMLValue])
    case table([String: TOMLValue])
}

/// Enough TOML for the config file: comments, tables, arrays of tables, dotted keys, all four
/// string forms, numbers, booleans, arrays and inline tables. Dates and times are rejected.
public enum TOML {
    /// Parse a document into its root table.
    public static func parse(_ text: String) throws -> [String: TOMLValue] {
        var parser = Parser(text)
        return try parser.document()
    }

    /// A string as a TOML value: literal when that avoids escapes, otherwise basic.
    public static func quote(_ string: String) -> String {
        let needsEscapes = string.unicodeScalars.contains { scalar in
            scalar == "'" || (scalar.value < 0x20 && scalar != "\t") || scalar.value == 0x7F
        }
        if !needsEscapes, string.contains("\"") || string.contains("\\") {
            return "'\(string)'"
        }
        var out = "\""
        for scalar in string.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\t": out += "\\t"
            case "\r": out += "\\r"
            case "\u{8}": out += "\\b"
            case "\u{C}": out += "\\f"
            default:
                if scalar.value < 0x20 || scalar.value == 0x7F {
                    out += String(format: "\\u%04X", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }

    /// A float as a TOML value, always with a decimal point or exponent so it reads back as a float.
    public static func format(_ value: Double) -> String {
        if value.isNaN { return "nan" }
        if value.isInfinite { return value < 0 ? "-inf" : "inf" }
        let text = "\(value)"
        return text.contains(where: { $0 == "." || $0 == "e" }) ? text : text + ".0"
    }
}

/// A table while parsing. A class, so headers can reopen a table and keep adding to it.
private final class Table {
    var entries: [String: Node] = [:]

    func value() -> [String: TOMLValue] {
        entries.mapValues { node in
            switch node {
            case .value(let value): value
            case .table(let table): .table(table.value())
            case .tables(let tables): .array(tables.map { .table($0.value()) })
            }
        }
    }
}

private enum Node {
    case value(TOMLValue)
    case table(Table)
    case tables([Table])
}

private struct Parser {
    let scalars: [Unicode.Scalar]
    var index = 0
    var line = 1

    init(_ text: String) {
        scalars = Array(text.unicodeScalars)
    }

    var peek: Unicode.Scalar? { peek(0) }

    func peek(_ offset: Int) -> Unicode.Scalar? {
        index + offset < scalars.count ? scalars[index + offset] : nil
    }

    func fail(_ message: String) -> AppError {
        AppError("line \(line): \(message)")
    }

    mutating func advance(_ count: Int = 1) {
        for _ in 0..<count where index < scalars.count {
            if scalars[index] == "\n" { line += 1 }
            index += 1
        }
    }

    func at(_ text: String) -> Bool {
        text.unicodeScalars.enumerated().allSatisfy { offset, scalar in peek(offset) == scalar }
    }

    mutating func skipSpaces() {
        while let c = peek, c == " " || c == "\t" { advance() }
    }

    mutating func skipComment() {
        guard peek == "#" else { return }
        while let c = peek, c != "\n" { advance() }
    }

    /// Whitespace, comments and newlines, as allowed between array elements.
    mutating func skipBlank() {
        while let c = peek {
            if c == " " || c == "\t" || c == "\n" || c == "\r" {
                advance()
            } else if c == "#" {
                skipComment()
            } else {
                return
            }
        }
    }

    /// The rest of a line after a key/value or header: spaces, maybe a comment, then a newline.
    mutating func endLine() throws {
        skipSpaces()
        skipComment()
        if peek == nil { return }
        if at("\r\n") {
            advance(2)
        } else if peek == "\n" {
            advance()
        } else {
            throw fail("unexpected text after a value")
        }
    }

    mutating func document() throws -> [String: TOMLValue] {
        let root = Table()
        var current = root
        while peek != nil {
            skipSpaces()
            guard let c = peek else { break }
            if c == "\n" || c == "\r" || c == "#" {
                try endLine()
                continue
            }
            if at("[[") {
                advance(2)
                skipSpaces()
                let path = try key()
                skipSpaces()
                guard at("]]") else { throw fail("expected ]] to close the table header") }
                advance(2)
                current = try appendTable(to: root, at: path)
            } else if c == "[" {
                advance()
                skipSpaces()
                let path = try key()
                skipSpaces()
                guard peek == "]" else { throw fail("expected ] to close the table header") }
                advance()
                current = try table(in: root, at: path)
            } else {
                try keyValue(into: current)
            }
            try endLine()
        }
        return root.value()
    }

    mutating func keyValue(into table: Table) throws {
        let path = try key()
        skipSpaces()
        guard peek == "=" else { throw fail("expected = after \(path.joined(separator: "."))") }
        advance()
        skipSpaces()
        let value = try value()
        let parent = try self.table(in: table, at: Array(path.dropLast()))
        let name = path[path.count - 1]
        if parent.entries[name] != nil {
            throw fail("\(path.joined(separator: ".")) is defined twice")
        }
        parent.entries[name] = .value(value)
    }

    /// A dotted key, e.g. `services.metrics`.
    mutating func key() throws -> [String] {
        var parts = [try simpleKey()]
        while true {
            skipSpaces()
            guard peek == "." else { return parts }
            advance()
            skipSpaces()
            parts.append(try simpleKey())
        }
    }

    mutating func simpleKey() throws -> String {
        switch peek {
        case "\"": return try basicString()
        case "'": return try literalString()
        default:
            var key = ""
            while let c = peek, Self.isBareKey(c) {
                key.unicodeScalars.append(c)
                advance()
            }
            guard !key.isEmpty else { throw fail("expected a key") }
            return key
        }
    }

    static func isBareKey(_ c: Unicode.Scalar) -> Bool {
        switch c {
        case "A"..."Z", "a"..."z", "0"..."9", "_", "-": true
        default: false
        }
    }

    /// Walk to the table at `path`, stepping into the newest entry of any array of tables.
    func table(in root: Table, at path: [String]) throws -> Table {
        var table = root
        for name in path {
            switch table.entries[name] {
            case nil:
                let child = Table()
                table.entries[name] = .table(child)
                table = child
            case .table(let child):
                table = child
            case .tables(let tables):
                table = tables[tables.count - 1]
            case .value:
                throw fail("\(name) is already a value, not a table")
            }
        }
        return table
    }

    func appendTable(to root: Table, at path: [String]) throws -> Table {
        let parent = try table(in: root, at: Array(path.dropLast()))
        let name = path[path.count - 1]
        let child = Table()
        switch parent.entries[name] {
        case nil: parent.entries[name] = .tables([child])
        case .tables(let tables): parent.entries[name] = .tables(tables + [child])
        default: throw fail("\(path.joined(separator: ".")) is not an array of tables")
        }
        return child
    }

    mutating func value() throws -> TOMLValue {
        switch peek {
        case nil: throw fail("expected a value")
        case "\"": return .string(at("\"\"\"") ? try multilineString(literal: false) : try basicString())
        case "'": return .string(at("'''") ? try multilineString(literal: true) : try literalString())
        case "[": return try array()
        case "{": return try inlineTable()
        default: return try scalar()
        }
    }

    mutating func basicString() throws -> String {
        advance()
        var out = ""
        while true {
            guard let c = peek, c != "\n" else { throw fail("unterminated string") }
            if c == "\"" {
                advance()
                return out
            }
            if c == "\\" {
                out.unicodeScalars.append(try escape())
            } else {
                out.unicodeScalars.append(c)
                advance()
            }
        }
    }

    mutating func literalString() throws -> String {
        advance()
        var out = ""
        while true {
            guard let c = peek, c != "\n" else { throw fail("unterminated string") }
            advance()
            if c == "'" { return out }
            out.unicodeScalars.append(c)
        }
    }

    mutating func multilineString(literal: Bool) throws -> String {
        let quote: Unicode.Scalar = literal ? "'" : "\""
        advance(3)
        // A newline straight after the opening quotes is not part of the string.
        if at("\r\n") { advance(2) } else if peek == "\n" { advance() }
        var out = ""
        while true {
            guard let c = peek else { throw fail("unterminated multi-line string") }
            if c == quote, peek(1) == quote, peek(2) == quote {
                // Up to two more quotes right before the closing three belong to the string.
                var run = 3
                while run < 5, peek(run) == quote { run += 1 }
                for _ in 3..<run { out.unicodeScalars.append(quote) }
                advance(run)
                return out
            }
            if !literal, c == "\\" {
                if lineEndingBackslash() {
                    skipBlank()
                } else {
                    out.unicodeScalars.append(try escape())
                }
                continue
            }
            out.unicodeScalars.append(c)
            advance()
        }
    }

    /// Whether the backslash here ends its line, which trims the newline and leading whitespace.
    mutating func lineEndingBackslash() -> Bool {
        var offset = 1
        while let c = peek(offset), c == " " || c == "\t" { offset += 1 }
        guard let c = peek(offset), c == "\n" || c == "\r" else { return false }
        advance(offset)
        return true
    }

    mutating func escape() throws -> Unicode.Scalar {
        advance()
        guard let c = peek else { throw fail("unterminated string") }
        advance()
        switch c {
        case "b": return "\u{8}"
        case "t": return "\t"
        case "n": return "\n"
        case "f": return "\u{C}"
        case "r": return "\r"
        case "e": return "\u{1B}"
        case "\"": return "\""
        case "\\": return "\\"
        case "u", "U":
            let length = c == "u" ? 4 : 8
            var hex = ""
            for _ in 0..<length {
                guard let digit = peek else { break }
                hex.unicodeScalars.append(digit)
                advance()
            }
            guard let code = UInt32(hex, radix: 16), let scalar = Unicode.Scalar(code) else {
                throw fail("invalid unicode escape \\\(c)\(hex)")
            }
            return scalar
        default:
            throw fail("invalid escape \\\(c)")
        }
    }

    mutating func array() throws -> TOMLValue {
        advance()
        var items: [TOMLValue] = []
        while true {
            skipBlank()
            if peek == "]" {
                advance()
                return .array(items)
            }
            items.append(try value())
            skipBlank()
            if peek == "," {
                advance()
            } else if peek == "]" {
                advance()
                return .array(items)
            } else {
                throw fail("expected , or ] in an array")
            }
        }
    }

    mutating func inlineTable() throws -> TOMLValue {
        advance()
        let table = Table()
        skipBlank()
        if peek == "}" {
            advance()
            return .table([:])
        }
        while true {
            skipBlank()
            try keyValue(into: table)
            skipBlank()
            if peek == "," {
                advance()
            } else if peek == "}" {
                advance()
                return .table(table.value())
            } else {
                throw fail("expected , or } in an inline table")
            }
        }
    }

    /// A boolean or number, read up to the next delimiter.
    mutating func scalar() throws -> TOMLValue {
        var token = ""
        while let c = peek, !" \t\r\n,]}#".unicodeScalars.contains(c) {
            token.unicodeScalars.append(c)
            advance()
        }
        switch token {
        case "true": return .bool(true)
        case "false": return .bool(false)
        case "inf", "+inf": return .float(.infinity)
        case "-inf": return .float(-.infinity)
        case "nan", "+nan", "-nan": return .float(.nan)
        default: break
        }
        if token.contains(":") || token.range(of: #"^\d{4}-\d{2}-\d{2}"#, options: .regularExpression) != nil {
            throw fail("dates and times aren't supported")
        }
        let digits = token.replacingOccurrences(of: "_", with: "")
        for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] where digits.hasPrefix(prefix) {
            guard let value = Int(digits.dropFirst(2), radix: radix) else { break }
            return .integer(value)
        }
        if digits.contains(where: { $0 == "." || $0 == "e" || $0 == "E" }) {
            if let value = Double(digits) { return .float(value) }
        } else if let value = Int(digits) {
            return .integer(value)
        }
        throw fail(token.isEmpty ? "expected a value" : "\(token) isn't a valid value")
    }
}
