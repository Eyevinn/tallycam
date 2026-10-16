import Foundation

/// The torch probe's analysis (docs/GENLOCK.md, "Torch probe"). The phone
/// lies above a white sheet and switches its own torch at times swept across
/// the frame phase. With a short shutter the rolling shutter shows each switch
/// as an edge at some row of one frame. Per switch:
///
///     command + torch latency = frame stamp + first-row offset + row × row time
///
/// so the edge row is linear in (command - stamp): the slope gives the row time
/// (and the readout time), the intercept "torch latency - first-row offset" (one
/// number: the two cannot be told apart on one phone), and the scatter around
/// the line the jitter of the torch command. The edge's width gives the torch's
/// rise time; ripple along lit frames shows PWM.
///
/// Pure, so it is unit tested without a camera.
enum TorchProbeAnalysis {
    enum Edge: String {
        case on
        case off
    }

    struct Command: Equatable {
        let edge: Edge
        /// Host clock, around the torch setter.
        let beforeNs: Int64
        let afterNs: Int64

        var midNs: Int64 {
            beforeNs + (afterNs - beforeNs) / 2
        }
    }

    /// One frame: its capture stamp (host clock) and the mean luma of every
    /// line along the readout axis, in buffer order.
    struct Frame {
        let stampNs: Int64
        let profile: [Float]
    }

    /// The edge found in one frame. Rows count in readout order (0 is the
    /// first line read).
    struct Crossing: Equatable {
        /// Where the normalised level crosses 0.5, fractional row.
        let row: Double
        /// Rows between the 10% and 90% crossings.
        let widthRows: Double
        /// True when the readout runs from the buffer's last line to its first.
        let reversed: Bool
    }

    struct Event {
        let command: Command
        let stampNs: Int64
        let crossing: Crossing

        /// Command time after the frame stamp, ns.
        var commandAfterStampNs: Double {
            Double(command.midNs - stampNs)
        }
    }

    /// edge row = (command - stamp + intercept) / row time
    struct Fit: Equatable {
        let rowTimeNs: Double
        /// Standard error of the row time: the jitter spread over the
        /// frame phase the switches cover.
        let rowTimeErrorNs: Double
        /// Torch latency minus the stamp's offset to the first row, ns.
        let interceptNs: Double
        let residualRmsNs: Double
        let residualMaxNs: Double
        /// Events used, after outliers are dropped.
        let used: Int
        let total: Int
    }

    struct Levels: Equatable {
        let dark: Float
        let bright: Float

        var contrast: Float {
            bright - dark
        }
    }

    struct Ripple: Equatable {
        /// Median over lit frames of the detrended profile's standard
        /// deviation, relative to the frame mean.
        let litRelative: Double
        /// The same over dark frames: the noise floor.
        let darkRelative: Double
        /// Median zero crossings of the detrended profile per lit frame.
        let litCrossings: Double
    }

    struct Result {
        let levels: Levels
        let rows: Int
        let events: [Event]
        let on: Fit?
        let off: Fit?
        let ripple: Ripple?
        let reversed: Bool?
        /// Commands with no edge in a frame (it fell between two frames'
        /// readouts, or was not seen).
        let missedOn: Int
        let missedOff: Int
    }

    /// Lowest contrast (8-bit luma) between torch off and on worth analysing.
    static let minimumContrast: Float = 12

    // MARK: Levels

    /// Dark and bright luma from the frames' means (5th and 95th percentile).
    static func levels(_ frames: [Frame]) -> Levels? {
        let means = frames.map { mean($0.profile) }.sorted()
        guard means.count >= 10 else {
            return nil
        }
        let dark = means[means.count * 5 / 100]
        let bright = means[min(means.count - 1, means.count * 95 / 100)]
        return Levels(dark: dark, bright: bright)
    }

    // MARK: One frame

    /// The torch's `edge` inside one frame, if the frame holds one: some lines
    /// read before the switch and some after.
    static func crossing(_ profile: [Float], levels: Levels, edge: Edge) -> Crossing? {
        let count = profile.count
        guard count >= 32, levels.contrast > 0 else {
            return nil
        }
        let level = smooth(profile, radius: 4).map { Double(($0 - levels.dark) / levels.contrast) }
        guard let low = level.min(), let high = level.max(), low < 0.2, high > 0.8 else {
            return nil
        }
        let band = max(4, count / 20)
        let head = level.prefix(band).reduce(0, +) / Double(band)
        let tail = level.suffix(band).reduce(0, +) / Double(band)
        // In readout order the level goes from "before" to "after" the switch.
        let headDark = head < 0.2 && tail > 0.8
        let headBright = head > 0.8 && tail < 0.2
        guard headDark || headBright else {
            return nil
        }
        // On: dark then bright in readout order; off: bright then dark.
        let reversed = (edge == .on) != headDark
        let ordered = reversed ? Array(level.reversed()) : level
        let rising = edge == .on
        guard let mid = firstCrossing(ordered, at: 0.5, rising: rising),
              let start = firstCrossing(ordered, at: rising ? 0.1 : 0.9, rising: rising),
              let end = firstCrossing(ordered, at: rising ? 0.9 : 0.1, rising: rising)
        else {
            return nil
        }
        return Crossing(row: mid, widthRows: max(0, end - start), reversed: reversed)
    }

    // MARK: The run

    static func analyze(frames: [Frame], commands: [Command], frameDurationNs: Double) -> Result? {
        let frames = frames.sorted { $0.stampNs < $1.stampNs }
        guard let levels = levels(frames), levels.contrast >= minimumContrast,
              let rows = frames.first?.profile.count
        else {
            return nil
        }
        let commands = commands.sorted { $0.beforeNs < $1.beforeNs }
        var events: [Event] = []
        var missed: [Edge: Int] = [.on: 0, .off: 0]
        for (index, command) in commands.enumerated() {
            // The frame holding the edge starts at most two frames before the
            // command and comes before the next command.
            let from = command.beforeNs - Int64(2 * frameDurationNs)
            let until = index + 1 < commands.count ? commands[index + 1].beforeNs : command.afterNs + 1_000_000_000
            let found = frames.lazy
                .filter { $0.stampNs >= from && $0.stampNs < until }
                .compactMap { frame in
                    crossing(frame.profile, levels: levels, edge: command.edge).map {
                        Event(command: command, stampNs: frame.stampNs, crossing: $0)
                    }
                }
                .first
            if let found {
                events.append(found)
            } else {
                missed[command.edge, default: 0] += 1
            }
        }
        let reversals = events.map(\.crossing.reversed)
        let reversed = reversals.isEmpty ? nil : reversals.filter { $0 }.count * 2 > reversals.count
        return Result(levels: levels, rows: rows, events: events,
                      on: fit(events.filter { $0.command.edge == .on }),
                      off: fit(events.filter { $0.command.edge == .off }),
                      ripple: ripple(frames, levels: levels), reversed: reversed,
                      missedOn: missed[.on] ?? 0, missedOff: missed[.off] ?? 0)
    }

    /// Least squares of edge row on (command - stamp), refitted once without
    /// the events more than three standard deviations off the line.
    static func fit(_ events: [Event]) -> Fit? {
        let points = events.map { (x: $0.commandAfterStampNs, y: $0.crossing.row) }
        guard var line = line(points) else {
            return nil
        }
        let sigma = rms(points.map { $0.y - (line.slope * $0.x + line.intercept) })
        let kept = points.filter { abs($0.y - (line.slope * $0.x + line.intercept)) <= max(3 * sigma, 1) }
        if kept.count < points.count, let refit = self.line(kept) {
            line = refit
        }
        guard line.slope != 0 else {
            return nil
        }
        let rowTime = 1 / line.slope
        let rowResiduals = kept.map { $0.y - (line.slope * $0.x + line.intercept) }
        let residuals = rowResiduals.map { $0 * abs(rowTime) }
        let mx = kept.map(\.x).reduce(0, +) / Double(kept.count)
        let sxx = kept.map { ($0.x - mx) * ($0.x - mx) }.reduce(0, +)
        let slopeError = kept.count > 2 && sxx > 0
            ? (rowResiduals.map { $0 * $0 }.reduce(0, +) / Double(kept.count - 2) / sxx).squareRoot() : 0
        return Fit(rowTimeNs: rowTime, rowTimeErrorNs: slopeError * rowTime * rowTime,
                   interceptNs: line.intercept * rowTime,
                   residualRmsNs: rms(residuals), residualMaxNs: residuals.map(abs).max() ?? 0,
                   used: kept.count, total: points.count)
    }

    static func ripple(_ frames: [Frame], levels: Levels) -> Ripple? {
        var lit: [Double] = []
        var dark: [Double] = []
        var crossings: [Double] = []
        for frame in frames {
            let level = (mean(frame.profile) - levels.dark) / levels.contrast
            let detrended = zip(frame.profile, smooth(frame.profile, radius: 16)).map { Double($0 - $1) }
            let relative = rms(detrended) / Double(max(1, mean(frame.profile)))
            // Only frames lit (or dark) all the way through.
            let low = frame.profile.min().map { ($0 - levels.dark) / levels.contrast } ?? 0
            let high = frame.profile.max().map { ($0 - levels.dark) / levels.contrast } ?? 1
            if level > 0.9, low > 0.8 {
                lit.append(relative)
                crossings.append(Double(zip(detrended, detrended.dropFirst()).filter { ($0 < 0) != ($1 < 0) }.count))
            } else if level < 0.1, high < 0.2 {
                dark.append(relative)
            }
        }
        guard !lit.isEmpty, !dark.isEmpty else {
            return nil
        }
        return Ripple(litRelative: median(lit), darkRelative: median(dark), litCrossings: median(crossings))
    }

    // MARK: Helpers

    static func smooth(_ values: [Float], radius: Int) -> [Float] {
        guard radius > 0, values.count > 1 else {
            return values
        }
        var prefix = [Double](repeating: 0, count: values.count + 1)
        for (index, value) in values.enumerated() {
            prefix[index + 1] = prefix[index] + Double(value)
        }
        return values.indices.map { index in
            let low = max(0, index - radius)
            let high = min(values.count, index + radius + 1)
            return Float((prefix[high] - prefix[low]) / Double(high - low))
        }
    }

    /// The first fractional index where `values` crosses `threshold` in the
    /// given direction, linearly interpolated.
    static func firstCrossing(_ values: [Double], at threshold: Double, rising: Bool) -> Double? {
        for index in 1 ..< max(1, values.count) {
            let a = values[index - 1]
            let b = values[index]
            let crosses = rising ? (a < threshold && b >= threshold) : (a > threshold && b <= threshold)
            if crosses {
                return Double(index - 1) + (threshold - a) / (b - a)
            }
        }
        return nil
    }

    private static func line(_ points: [(x: Double, y: Double)]) -> (slope: Double, intercept: Double)? {
        guard points.count >= 3 else {
            return nil
        }
        let n = Double(points.count)
        let mx = points.map(\.x).reduce(0, +) / n
        let my = points.map(\.y).reduce(0, +) / n
        let sxx = points.map { ($0.x - mx) * ($0.x - mx) }.reduce(0, +)
        let sxy = points.map { ($0.x - mx) * ($0.y - my) }.reduce(0, +)
        guard sxx > 0 else {
            return nil
        }
        let slope = sxy / sxx
        return (slope, my - slope * mx)
    }

    private static func mean(_ values: [Float]) -> Float {
        values.isEmpty ? 0 : values.reduce(0, +) / Float(values.count)
    }

    private static func rms(_ values: [Double]) -> Double {
        values.isEmpty ? 0 : (values.map { $0 * $0 }.reduce(0, +) / Double(values.count)).squareRoot()
    }

    private static func median(_ values: [Double]) -> Double {
        let sorted = values.sorted()
        return sorted.isEmpty ? 0 : sorted[sorted.count / 2]
    }
}
