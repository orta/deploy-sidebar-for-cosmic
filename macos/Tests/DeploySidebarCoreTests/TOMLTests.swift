// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Testing

@Test func scalarsAndComments() throws {
    let doc = try TOML.parse("""
        # comment
        a = 1 # trailing
        b = -2_000
        c = 0.5
        d = 5e-3
        e = true
        f = 0xff
        g = inf
        "quoted key" = "v"
        """)
    #expect(doc["a"] == .integer(1))
    #expect(doc["b"] == .integer(-2000))
    #expect(doc["c"] == .float(0.5))
    #expect(doc["d"] == .float(0.005))
    #expect(doc["e"] == .bool(true))
    #expect(doc["f"] == .integer(255))
    #expect(doc["g"] == .float(.infinity))
    #expect(doc["quoted key"] == .string("v"))
}

@Test func strings() throws {
    let doc = try TOML.parse(#"""
        basic = "tab\there \"q\" \u00e9"
        literal = 'C:\path "as is"'
        multi = """
        line one
        line two\
            joined"""
        multiLiteral = '''
        raw \n ''quoted'''''
        """#)
    #expect(doc["basic"] == .string("tab\there \"q\" é"))
    #expect(doc["literal"] == .string(#"C:\path "as is""#))
    #expect(doc["multi"] == .string("line one\nline twojoined"))
    #expect(doc["multiLiteral"] == .string("raw \\n ''quoted''"))
}

@Test func tablesAndArrays() throws {
    let doc = try TOML.parse("""
        top = [1, 2,
          3, # comment
        ]
        inline = { x = 1, y.z = "two" }

        [server]
        host = "h"

        [server.tls]
        on = true

        [[services]]
        name = "a"

        [[services.metrics]]
        label = "m1"

        [[services.metrics]]
        label = "m2"

        [[services]]
        name = "b"
        """)
    #expect(doc["top"] == .array([.integer(1), .integer(2), .integer(3)]))
    #expect(doc["inline"] == .table(["x": .integer(1), "y": .table(["z": .string("two")])]))
    #expect(doc["server"] == .table(["host": .string("h"), "tls": .table(["on": .bool(true)])]))
    #expect(doc["services"] == .array([
        .table(["name": .string("a"), "metrics": .array([
            .table(["label": .string("m1")]),
            .table(["label": .string("m2")]),
        ])]),
        .table(["name": .string("b")]),
    ]))
}

@Test func crlfLineEndings() throws {
    let doc = try TOML.parse("a = 1\r\nb = 'x'\r\n")
    #expect(doc == ["a": .integer(1), "b": .string("x")])
}

@Test func errorsHaveLineNumbers() {
    #expect(throws: AppError("line 2: a is defined twice")) { try TOML.parse("a = 1\na = 2") }
    #expect(throws: AppError("line 1: unterminated string")) { try TOML.parse("a = \"oops\nb = 1") }
    #expect(throws: AppError("line 1: unexpected text after a value")) { try TOML.parse("a = 1 b") }
    #expect(throws: AppError("line 1: dates and times aren't supported")) { try TOML.parse("a = 1979-05-27") }
    #expect(throws: AppError("line 1: expected = after a")) { try TOML.parse("a 1") }
}

@Test func quotingRoundTrips() throws {
    for value in ["plain", #"has "double" quotes"#, "it's", #"both ' and ""#, "new\nline", #"back\slash"#, "tab\t"] {
        let doc = try TOML.parse("v = \(TOML.quote(value))")
        #expect(doc["v"] == .string(value))
    }
    #expect(TOML.quote(#"job="api""#) == #"'job="api"'"#)
    #expect(TOML.format(5) == "5.0")
    #expect(TOML.format(0.2) == "0.2")
    #expect(TOML.format(1e-5) == "1e-05")
}
