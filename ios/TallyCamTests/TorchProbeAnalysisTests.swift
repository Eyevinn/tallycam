import XCTest

/// The torch probe's analysis on a simulated rolling shutter: lines exposed
/// one after the other, a torch that switches some time after its command.
final class TorchProbeAnalysisTests: XCTestCase {
    struct Camera {
        var lines = 1000
        var lineNs = 20_000.0
        var frameNs = 33_333_333.0
        /// From the stamp to the middle of the first line's exposure.
        var firstLineNs = 2_000_000.0
        var shutterNs = 500_000.0
        var dark: Float = 20
        var bright: Float = 180
        var reversed = false
    }

    /// Commands as the probe issues them, and the instants the light changes.
    func schedule(cycles: Int, latencyNs: Double, jitterNs: Double)
        -> (commands: [TorchProbeAnalysis.Command], switches: [(on: Double, off: Double)])
    {
        var commands: [TorchProbeAnalysis.Command] = []
        var switches: [(on: Double, off: Double)] = []
        var seed: UInt64 = 1
        func jitter() -> Double {
            seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return (Double(seed >> 11) / Double(1 << 53) * 2 - 1) * jitterNs
        }
        for cycle in 0 ..< cycles {
            let phase = (Double(cycle) * 0.618_033_988_75).truncatingRemainder(dividingBy: 1) * 33_333_333
            let on = 1e9 + Double(cycle) * 600e6 + phase
            let off = on + 300e6
            commands.append(.init(edge: .on, beforeNs: Int64(on), afterNs: Int64(on) + 200_000))
            commands.append(.init(edge: .off, beforeNs: Int64(off), afterNs: Int64(off) + 200_000))
            switches.append((on + 100_000 + latencyNs + jitter(), off + 100_000 + latencyNs + jitter()))
        }
        return (commands, switches)
    }

    func frames(_ camera: Camera, switches: [(on: Double, off: Double)], durationNs: Double)
        -> [TorchProbeAnalysis.Frame]
    {
        func lit(from start: Double, to end: Double) -> Double {
            switches.map { max(0, min(end, $0.off) - max(start, $0.on)) }.reduce(0, +) / (end - start)
        }
        return stride(from: 0.0, to: durationNs, by: camera.frameNs).map { stamp in
            var profile = (0 ..< camera.lines).map { line -> Float in
                let middle = stamp + camera.firstLineNs + Double(line) * camera.lineNs
                let level = lit(from: middle - camera.shutterNs / 2, to: middle + camera.shutterNs / 2)
                return camera.dark + Float(level) * (camera.bright - camera.dark)
            }
            if camera.reversed {
                profile.reverse()
            }
            return .init(stampNs: Int64(stamp), profile: profile)
        }
    }

    func testRecoversLineTimeLatencyAndJitter() throws {
        let camera = Camera()
        let latency = 30e6
        let (commands, switches) = schedule(cycles: 40, latencyNs: latency, jitterNs: 300_000)
        let result = try XCTUnwrap(TorchProbeAnalysis.analyze(
            frames: frames(camera, switches: switches, durationNs: 26e9),
            commands: commands, frameDurationNs: camera.frameNs
        ))
        XCTAssertEqual(result.reversed, false)
        XCTAssertGreaterThan(result.events.count, 30)
        for fit in [result.on, result.off] {
            let fit = try XCTUnwrap(fit)
            // ±0.3 ms of jitter over the 20 ms the edges cover: about 1%.
            XCTAssertEqual(fit.rowTimeNs, camera.lineNs, accuracy: 4 * fit.rowTimeErrorNs)
            XCTAssertGreaterThan(fit.rowTimeErrorNs, 50)
            XCTAssertLessThan(fit.rowTimeErrorNs, 400)
            // Latency minus the first line's offset.
            XCTAssertEqual(fit.interceptNs, latency - camera.firstLineNs, accuracy: 200_000)
            // Uniform ±0.3 ms: rms 0.17 ms.
            XCTAssertEqual(fit.residualRmsNs, 300_000 / 3.0.squareRoot(), accuracy: 60000)
        }
        // Edges in the blanking between readouts are not seen: 20 of 33 ms.
        XCTAssertGreaterThan(result.missedOn + result.missedOff, 0)
    }

    func testReversedReadout() throws {
        var camera = Camera()
        camera.reversed = true
        let (commands, switches) = schedule(cycles: 30, latencyNs: 25e6, jitterNs: 0)
        let result = try XCTUnwrap(TorchProbeAnalysis.analyze(
            frames: frames(camera, switches: switches, durationNs: 20e9),
            commands: commands, frameDurationNs: camera.frameNs
        ))
        XCTAssertEqual(result.reversed, true)
        let fit = try XCTUnwrap(result.on)
        XCTAssertEqual(fit.rowTimeNs, camera.lineNs, accuracy: 0.5)
        XCTAssertEqual(fit.interceptNs, 25e6 - camera.firstLineNs, accuracy: 100_000)
    }

    func testEdgeWidthFollowsTheShutter() throws {
        var camera = Camera()
        camera.shutterNs = 2_000_000
        let (commands, switches) = schedule(cycles: 30, latencyNs: 25e6, jitterNs: 0)
        let result = try XCTUnwrap(TorchProbeAnalysis.analyze(
            frames: frames(camera, switches: switches, durationNs: 20e9),
            commands: commands, frameDurationNs: camera.frameNs
        ))
        // 10 to 90% of a 2 ms shutter is 1.6 ms, 80 lines of 20 us.
        let widths = result.events.map(\.crossing.widthRows)
        XCTAssertFalse(widths.isEmpty)
        for width in widths {
            XCTAssertEqual(width, 80, accuracy: 10)
        }
    }

    func testNoContrastIsNoResult() {
        var camera = Camera()
        camera.bright = camera.dark + 5
        let (commands, switches) = schedule(cycles: 10, latencyNs: 25e6, jitterNs: 0)
        XCTAssertNil(TorchProbeAnalysis.analyze(
            frames: frames(camera, switches: switches, durationNs: 7e9),
            commands: commands, frameDurationNs: camera.frameNs
        ))
    }

    func testCrossingInterpolates() {
        let values: [Double] = [0, 0, 0.25, 0.75, 1, 1]
        XCTAssertEqual(TorchProbeAnalysis.firstCrossing(values, at: 0.5, rising: true) ?? -1, 2.5, accuracy: 1e-9)
        XCTAssertNil(TorchProbeAnalysis.firstCrossing(values, at: 0.5, rising: false))
    }
}
