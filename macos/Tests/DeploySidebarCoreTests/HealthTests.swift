// SPDX-License-Identifier: GPL-3.0

@testable import DeploySidebarCore
import Testing

private func metric(_ worse: Direction, floor: Double) -> Metric {
    Metric(label: "", expr: "", unit: .seconds, worse: worse, floor: floor)
}

@Test func higherIsWorse() {
    let m = metric(.higher, floor: 0)
    #expect(Health.assess(m, baseline: 1, current: 1.1) == .ok)
    #expect(Health.assess(m, baseline: 1, current: 1.3) == .warn)
    #expect(Health.assess(m, baseline: 1, current: 2) == .bad)
    #expect(Health.assess(m, baseline: nil, current: 2) == .unknown)
}

@Test func floorStopsTinyValuesTripling() {
    let m = metric(.higher, floor: 0.05)
    #expect(Health.assess(m, baseline: 0.001, current: 0.003) == .ok)
    // Against a zero baseline the floor stands in, so 0.1 is double it.
    #expect(Health.assess(m, baseline: 0, current: 0.1) == .bad)
}

@Test func lowerIsWorse() {
    let m = metric(.lower, floor: 1)
    #expect(Health.assess(m, baseline: 100, current: 95) == .ok)
    #expect(Health.assess(m, baseline: 100, current: 50) == .bad)
    #expect(Health.assess(m, baseline: 100, current: 0) == .bad)
    #expect(Health.assess(m, baseline: 0.5, current: 0) == .ok)
}

@Test func formatting() {
    #expect(Health.format(.seconds, 0.123) == "123ms")
    #expect(Health.format(.seconds, 1.5) == "1.50s")
    #expect(Health.format(.perSecond, 1234) == "1.2k/s")
    #expect(Health.format(.count, 42) == "42")
    #expect(Health.format(.count, 0.5) == "0.50")
    #expect(Health.format(.ratio, 0.025) == "2.5%")
    #expect(Health.format(.bytes, 734_003_200) == "700MB")
    #expect(Health.format(.bytes, 2_147_483_648) == "2.0GB")
    #expect(Health.delta(baseline: 100, current: 112) == "+12%")
    #expect(Health.delta(baseline: 100, current: 88) == "-12%")
    #expect(Health.delta(baseline: 0, current: 1) == nil)
    #expect(Health.duration(245) == "4m 05s")
    #expect(Health.duration(40) == "40s")
    #expect(Health.duration(-3) == "0s")
}
