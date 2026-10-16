import XCTest

/// The shared ownership vectors (core/tests/vectors/lcp_ownership.json):
/// the server messages through the app's parser, and the scenarios through
/// the core's state machine as the app calls it (UniFFI).
final class OwnershipVectorTests: XCTestCase {
    private func vectors() throws -> [String: JSONValue] {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "lcp_ownership", withExtension: "json"))
        return try JSONDecoder().decode([String: JSONValue].self, from: Data(contentsOf: url))
    }

    private func strings(_ value: JSONValue?) -> [String] {
        guard case let .array(items)? = value else {
            return []
        }
        return items.compactMap(\.string)
    }

    func testServerMessagesParse() throws {
        guard case let .array(messages)? = try vectors()["messages"] else {
            return XCTFail("no messages")
        }
        var parsed = 0
        for case let .object(vector) in messages where vector["direction"]?.string == "server_to_device" {
            let name = vector["name"]?.string ?? "?"
            let canonical = try XCTUnwrap(vector["canonical"]?.object, name)
            let text = try XCTUnwrap(String(data: JSONEncoder().encode(vector["json"]), encoding: .utf8))
            let message = try XCTUnwrap(LcpIncoming.parse(text), name)
            switch message {
            case let .own(_, groups, holderId, holderLabel):
                let holder = canonical["holder"]?.object
                XCTAssertEqual(groups, strings(canonical["groups"]), name)
                XCTAssertEqual(holderId, holder?["id"]?.string, name)
                XCTAssertEqual(holderLabel, holder?["label"]?.string, name)
            case let .release(_, groups):
                XCTAssertEqual(groups, strings(canonical["groups"]), name)
            case let .production(mainsHz, shutter):
                XCTAssertEqual(Double(mainsHz), canonical["mains_hz"]?.double, name)
                XCTAssertEqual(shutter, ShutterPolicy(canonical["shutter"]), name)
            default:
                XCTFail("\(name): parsed as \(message)")
            }
            parsed += 1
        }
        XCTAssertGreaterThanOrEqual(parsed, 7)
    }

    func testScenariosThroughTheCore() throws {
        guard case let .array(scenarios)? = try vectors()["scenarios"] else {
            return XCTFail("no scenarios")
        }
        XCTAssertGreaterThanOrEqual(scenarios.count, 10)
        for case let .object(scenario) in scenarios {
            let name = scenario["name"]?.string ?? "?"
            var machine = OwnershipMachine(groups: strings(scenario["groups"]), savedJson: nil)
            guard case let .array(steps)? = scenario["steps"] else {
                continue
            }
            for (i, item) in steps.enumerated() {
                guard case let .object(step) = item else {
                    continue
                }
                let at = "\(name) step \(i)"
                let group = step["group"]?.string ?? ""
                var result: String?
                switch step["do"]?.string {
                case "connect": machine.setConnected(connected: true)
                case "disconnect": machine.setConnected(connected: false)
                case "restart": machine = OwnershipMachine(groups: strings(scenario["groups"]),
                                                           savedJson: machine.saveJson())
                case "own":
                    let holder = step["holder"]?.object
                    let taken = machine.own(groups: strings(step["groups"]), holderId: holder?["id"]?.string ?? "",
                                            holderLabel: holder?["label"]?.string ?? "")
                    if step["taken"] != nil { XCTAssertEqual(taken.taken, strings(step["taken"]), at) }
                    if step["from_local"] != nil { XCTAssertEqual(taken.fromLocal, strings(step["from_local"]), at) }
                    if step["skipped"] != nil { XCTAssertEqual(taken.skipped, strings(step["skipped"]), at) }
                case "release":
                    let released = machine.release(groups: strings(step["groups"]))
                    if step["released"] != nil { XCTAssertEqual(released, strings(step["released"]), at) }
                case "new_server":
                    let released = machine.newServer()
                    if step["released"] != nil { XCTAssertEqual(released, strings(step["released"]), at) }
                case "remote_set": result = machine.checkRemote(group: group) == nil ? "changed" : "refused"
                case "local_change": result = Self.code(machine.localChange(group: group))
                case "local_auto": result = Self.code(machine.localAuto(group: group))
                case "take_local": result = Self.code(machine.takeLocal(group: group))
                default: XCTFail("\(at): unknown action")
                }
                if let want = step["result"]?.string {
                    // The FFI reports refusals as text; the vectors name them.
                    XCTAssertEqual(result, ["changed", "unchanged"].contains(want) ? want : "refused", at)
                }
                if case let .object(want)? = step["ownership"] {
                    let message = try JSONDecoder().decode([String: JSONValue].self,
                                                           from: Data(machine.messageJson().utf8))
                    XCTAssertEqual(message["groups"], .object(want), at)
                }
                if case let .object(indicators)? = step["indicators"] {
                    let view = machine.view()
                    for (group, want) in indicators {
                        XCTAssertEqual(view.first { $0.group == group }?.indicator, want.string, "\(at) \(group)")
                    }
                }
            }
        }
    }

    private static func code(_ change: OwnershipChange) -> String {
        switch change {
        case .changed: "changed"
        case .unchanged: "unchanged"
        case .refused: "refused"
        }
    }
}
