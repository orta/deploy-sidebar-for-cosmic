// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Testing

@Test func nearestPicksTheQuadrant() {
    #expect(Corner.nearest(x: 10, y: 10, width: 100, height: 50) == .topLeft)
    #expect(Corner.nearest(x: 90, y: 10, width: 100, height: 50) == .topRight)
    #expect(Corner.nearest(x: 10, y: 40, width: 100, height: 50) == .bottomLeft)
    #expect(Corner.nearest(x: 90, y: 40, width: 100, height: 50) == .bottomRight)
}

@Test func placementRoundTrips() {
    let placement = Placement(corner: .bottomLeft, output: "Built-in Retina Display")
    #expect(placement.toTOML().contains(#"corner = "bottom-left""#))
    #expect(Placement.parse(placement.toTOML()) == placement)
}

@Test func placementMatchesTheLinuxFormat() {
    // As written by the COSMIC app's `toml::to_string`.
    let placement = Placement.parse("corner = \"bottom-right\"\noutput = \"DP-3\"\n")
    #expect(placement == Placement(corner: .bottomRight, output: "DP-3"))
}

@Test func placementDefaultsToTopRight() {
    #expect(Placement.parse("") == Placement())
    #expect(Placement.parse("corner = \"middle\"") == Placement())
    #expect(Placement.parse("not toml [") == Placement())
}
