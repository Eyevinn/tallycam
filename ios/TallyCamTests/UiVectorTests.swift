import XCTest

/// The shared UI vectors (core/tests/vectors/ui_values.json, Android runs
/// the same): the values row, the side controls' value labels, the rockers'
/// tap steps and neutral values, and which group each side control runs.
final class UiVectorTests: XCTestCase {
    private func vectors() throws -> [String: JSONValue] {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "ui_values", withExtension: "json"))
        return try JSONDecoder().decode([String: JSONValue].self, from: Data(contentsOf: url))
    }

    private func cases(_ key: String) throws -> [[String: JSONValue]] {
        guard case let .array(items)? = try vectors()[key] else {
            XCTFail("no \(key)")
            return []
        }
        return items.compactMap(\.object)
    }

    private func descriptors() throws -> [String: ControlDescriptor] {
        let object = try XCTUnwrap(vectors()["descriptors"]?.object)
        var map: [String: ControlDescriptor] = [:]
        for (id, range) in object {
            let range = try XCTUnwrap(range.object)
            map[id] = try ControlDescriptor(id: id, label: id, group: "test",
                                            kind: .range(min: XCTUnwrap(range["min"]?.double),
                                                         max: XCTUnwrap(range["max"]?.double),
                                                         step: XCTUnwrap(range["step"]?.double)))
        }
        return map
    }

    /// Numbers compared rounded to 1e-6, strings as they are.
    private func normalised(_ set: [String: JSONValue]?) -> [String: String]? {
        set?.mapValues { value in
            if case let .number(x) = value {
                return String(format: "%.6f", (x * 1e6).rounded() / 1e6)
            }
            return value.string ?? "\(value)"
        }
    }

    private func sides(_ object: [String: JSONValue]?) -> Sides {
        Sides(left: object?["left"]?.string.flatMap(SideGroup.init(rawValue:)),
              right: object?["right"]?.string.flatMap(SideGroup.init(rawValue:)),
              leftHidden: object?["left_hidden"] == .bool(true),
              rightHidden: object?["right_hidden"] == .bool(true))
    }

    func testFormat() throws {
        for vector in try cases("format") {
            let text = try UiRules.formatText(width: Int(XCTUnwrap(vector["width"]?.double)),
                                              height: Int(XCTUnwrap(vector["height"]?.double)),
                                              fps: Int(XCTUnwrap(vector["fps"]?.double)))
            XCTAssertEqual(text, vector["expect"]?.string)
        }
    }

    func testValuesRow() throws {
        for vector in try cases("values_row") {
            let name = vector["name"]?.string ?? "?"
            let items = UiRules.valueItems(vector["values"]?.object ?? [:], format: vector["format"]?.string ?? "",
                                           warning: vector["warning"]?.string)
            guard case let .array(expected)? = vector["expect"] else {
                return XCTFail(name)
            }
            XCTAssertEqual(items.count, expected.count, name)
            for (item, expect) in zip(items, expected.compactMap(\.object)) {
                XCTAssertEqual(item.text, expect["text"]?.string, name)
                XCTAssertEqual(item.flag, expect["flag"]?.string, name)
                XCTAssertEqual(item.tone.rawValue, expect["tone"]?.string, name)
                XCTAssertEqual(item.group?.rawValue, expect["group"]?.string, name)
            }
        }
    }

    /// Numbers print with a point whatever the phone's language: the rules
    /// format with a fixed locale, never the current one.
    func testValuesRowIgnoresTheLocale() {
        let swedish = String(format: "%.2f", locale: Locale(identifier: "sv_SE"), 0.42)
        XCTAssertEqual(swedish, "0,42", "the test's premise")
        let items = UiRules.valueItems(["focus_mode": .string("manual"), "focus_position": .number(0.42),
                                        "exposure_mode": .string("auto"), "ev_bias": .number(-0.7)],
                                       format: "", warning: nil)
        XCTAssertEqual(items.map(\.text), ["-0.7 EV", "F 0.42"])
    }

    func testDropOrder() throws {
        guard case let .array(order)? = try vectors()["drop_order"] else {
            return XCTFail("no drop_order")
        }
        XCTAssertEqual(UiRules.dropOrder.map(\.rawValue), order.compactMap(\.string))
    }

    func testAdjustValue() throws {
        for vector in try cases("adjust_value") {
            let group = try XCTUnwrap(vector["group"]?.string.flatMap(ShadingGroup.init(rawValue:)))
            XCTAssertEqual(UiRules.adjustValueText(group, vector["values"]?.object ?? [:],
                                                   tint: vector["tint"] == .bool(true)),
                           vector["expect"]?.string)
        }
    }

    func testZoomFactorBitrateUptime() throws {
        for vector in try cases("zoom_factor") {
            XCTAssertEqual(try UiRules.zoomFactorText(XCTUnwrap(vector["factor"]?.double)), vector["expect"]?.string)
        }
        for vector in try cases("bitrate") {
            XCTAssertEqual(try UiRules.bitrate(kbps: Int(XCTUnwrap(vector["kbps"]?.double))), vector["expect"]?.string)
        }
        for vector in try cases("uptime") {
            XCTAssertEqual(try UiRules.uptime(seconds: XCTUnwrap(vector["seconds"]?.double)), vector["expect"]?.string)
        }
    }

    func testTapSteps() throws {
        let descriptors = try descriptors()
        for vector in try cases("steps") {
            let name = vector["name"]?.string ?? "?"
            let group = try XCTUnwrap(vector["group"]?.string.flatMap(SideGroup.init(rawValue:)), name)
            let set = UiRules.tapStep(group, vector["values"]?.object ?? [:], descriptors,
                                      tint: vector["tint"] == .bool(true),
                                      direction: Int(vector["direction"]?.double ?? 0), base: vector["base"]?.double)
            XCTAssertEqual(normalised(set), normalised(vector["expect"]?.object), name)
        }
    }

    func testNeutral() throws {
        for vector in try cases("neutral") {
            let group = try XCTUnwrap(vector["group"]?.string.flatMap(SideGroup.init(rawValue:)))
            let neutral = UiRules.neutral(group, vector["values"]?.object ?? [:], tint: vector["tint"] == .bool(true))
            let name = "\(group) \(vector["values"].map { "\($0)" } ?? "")"
            switch (neutral, vector["expect"]) {
            case (nil, .null?), (nil, nil):
                break
            case let (.set(values)?, .object(expect)?):
                XCTAssertEqual(normalised(values), normalised(expect["set"]?.object), name)
            case let (.command(command)?, .object(expect)?):
                XCTAssertEqual(command, expect["command"]?.string, name)
            default:
                XCTFail("\(name): \(String(describing: neutral))")
            }
        }
    }

    func testSides() throws {
        let object = try XCTUnwrap(vectors()["sides"]?.object)
        XCTAssertEqual(Sides.default, sides(object["default"]?.object))
        guard case let .array(items)? = object["cases"] else {
            return XCTFail("no cases")
        }
        for vector in items.compactMap(\.object) {
            let name = vector["name"]?.string ?? "?"
            let start = sides(vector["start"]?.object)
            var result = start
            if let choose = vector["choose"]?.object {
                let side: Side = choose["side"]?.string == "left" ? .left : .right
                result = try start.choose(side, XCTUnwrap(choose["group"]?.string.flatMap(SideGroup.init(rawValue:))))
            } else if let show = vector["show"]?.string {
                result = start.show(show == "left" ? .left : .right)
            } else if let hide = vector["hide"]?.string {
                result = start.hide(hide == "left" ? .left : .right)
            } else if let tap = vector["values_tap"]?.string {
                result = try start.fromValues(XCTUnwrap(SideGroup(rawValue: tap)))
            } else if case let .array(available)? = vector["available"] {
                result = start.resolved(available: available.compactMap { $0.string.flatMap(SideGroup.init(rawValue:)) })
            }
            XCTAssertEqual(result, sides(vector["expect"]?.object), name)
        }
    }
}
