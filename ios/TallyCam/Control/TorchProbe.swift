@preconcurrency import AVFoundation
import os
import UIKit

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "torch-probe")

/// The `torch_probe` command (docs/GENLOCK.md, "Torch probe"): switches the
/// torch on and off at times swept across the frame phase, records a luma
/// profile of every frame along the readout axis, and fits the result
/// (`TorchProbeAnalysis`). The phone lies a few cm above a white sheet, with
/// manual exposure and a short shutter. Results go to the log and to
/// Documents/torch-probe/ (Files app, or Finder with the phone connected).
///
/// Modes: `self` switches the torch and analyses this camera alone; `emit`
/// switches it for another camera to watch; `watch` only records, for the
/// pair analysis of an emitting phone's light (`tools/torch-pair`). Every
/// frame and command is also stamped in TAI, so two phones' files line up.
final class TorchProbe: @unchecked Sendable {
    enum Mode: String {
        case solo = "self"
        case emit
        case watch
    }

    struct Settings {
        var mode = Mode.solo
        var cycles = 40
        var onMs = 300.0
        var offMs = 300.0
    }

    struct FrameTime {
        let taiNs: Int64
        let synced: Bool
    }

    private let lock = NSLock()
    // Guarded by `lock`.
    private var recording = false
    private var frames: [TorchProbeAnalysis.Frame] = []
    private var frameTimes: [FrameTime] = []
    private var running = false

    var isRunning: Bool {
        lock.withLock { running }
    }

    /// Every captured frame, on the video queue. Costs one lock unless a
    /// probe records.
    func onFrame(_ sampleBuffer: CMSampleBuffer) {
        guard lock.withLock({ recording }),
              let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer)
        else {
            return
        }
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        guard pts.isValid, let profile = Self.profile(pixelBuffer) else {
            return
        }
        let stampNs = HostClock.ns(fromTicks: CMClockConvertHostTimeToSystemUnits(pts))
        let mapping = ClockMapping.at(hostNs: stampNs)
        lock.withLock {
            if recording {
                frames.append(.init(stampNs: stampNs, profile: profile))
                frameTimes.append(.init(taiNs: mapping.taiNs, synced: mapping.synced))
            }
        }
    }

    /// Mean luma per line across the readout. The sensor reads along its
    /// short side, which stays the buffer's short side after the connection
    /// rotates it: lines are rows in landscape, columns in portrait. Uses the
    /// middle half of each line, every fourth pixel.
    static func profile(_ buffer: CVPixelBuffer) -> [Float]? {
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard CVPixelBufferGetPlaneCount(buffer) >= 1,
              let base = CVPixelBufferGetBaseAddressOfPlane(buffer, 0)
        else {
            return nil
        }
        let width = CVPixelBufferGetWidthOfPlane(buffer, 0)
        let height = CVPixelBufferGetHeightOfPlane(buffer, 0)
        let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0)
        let luma = base.assumingMemoryBound(to: UInt8.self)
        let step = 4
        if width >= height {
            return (0 ..< height).map { y in
                let row = luma + y * stride
                var sum = 0
                var count = 0
                for x in Swift.stride(from: width / 4, to: width * 3 / 4, by: step) {
                    sum += Int(row[x])
                    count += 1
                }
                return Float(sum) / Float(max(1, count))
            }
        }
        var sums = [Int](repeating: 0, count: width)
        var count = 0
        for y in Swift.stride(from: height / 4, to: height * 3 / 4, by: step) {
            let row = luma + y * stride
            for x in 0 ..< width {
                sums[x] += Int(row[x])
            }
            count += 1
        }
        return sums.map { Float($0) / Float(max(1, count)) }
    }

    /// Runs the probe on the session queue. Returns at once; the result is
    /// reported through `report` when the run ends, about
    /// cycles × (on + off) + 2 s later.
    /// The most the recorded profiles may take.
    static let maxRecordingBytes = 100e6

    func start(controls c: CameraControls, settings: Settings,
               report: @escaping @Sendable (_ level: String, _ message: String) -> Void) throws
    {
        guard let device = c.device else {
            throw ControlError("no camera")
        }
        let switching = settings.mode != .watch
        // The torch belongs to the wide camera. Another lens (ultra wide)
        // films while the wide camera's torch switches, to see whether the
        // torch is held for the frames of the camera that runs or of its own.
        let torchDevice = device.hasTorch ? device
            : AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
        if switching, !(torchDevice.map { $0.hasTorch && $0.isTorchModeSupported(.on) } ?? false) {
            throw ControlError("no torch (mode watch records without one)")
        }
        guard device.exposureMode == .custom else {
            throw ControlError("set manual exposure first (a short shutter, 1/2000 or shorter, and a fixed ISO)")
        }
        guard (5 ... 400).contains(settings.cycles), (150 ... 2000).contains(settings.onMs),
              (150 ... 2000).contains(settings.offMs)
        else {
            throw ControlError("cycles 5..400, on_ms and off_ms 150..2000")
        }
        guard lock.withLock({ !running }) else {
            throw ControlError("a torch probe is already running")
        }
        let frameDurationNs = device.activeVideoMinFrameDuration.seconds * 1e9
        guard frameDurationNs > 0 else {
            throw ControlError("no frame rate")
        }
        // Every frame keeps a profile of one Float per line: refuse a run
        // that would hold more than this in memory (a long run at a high
        // frame rate would otherwise get the app killed).
        let dimensions = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
        let seconds = Double(settings.cycles) * (settings.onMs + settings.offMs) / 1000 + 2
        let bytes = seconds * 1e9 / frameDurationNs * Double(max(dimensions.width, dimensions.height)) * 4
        guard bytes <= Self.maxRecordingBytes else {
            throw ControlError(String(format: "too long a run for this frame rate (%.0f MB); fewer cycles or shorter",
                                      bytes / 1e6))
        }
        // The dark baseline needs the torch off. Before anything is set up,
        // so a failure here leaves no probe half started.
        if switching, let torchDevice, torchDevice.torchMode != .off {
            try torchDevice.lockForConfiguration()
            torchDevice.torchMode = .off
            torchDevice.unlockForConfiguration()
        }
        lock.withLock {
            running = true
            frames = []
            frameTimes = []
        }
        var warnings: [String] = []
        if device.exposureDuration.seconds > 0.002 {
            warnings.append(String(format: "shutter 1/%.0f smears the edge; 1/2000 or shorter is better",
                                   1 / device.exposureDuration.seconds))
        }
        if c.stabilization != .off {
            warnings.append("stabilization is on and moves lines in time; turn it off")
        }
        // Auto ISO would chase the torch. Given back at the end only if
        // nobody chose an exposure mode (or another lens) meanwhile.
        let autoIso = c.autoIso
        let modeChanges = c.exposureModeChanges
        c.autoIso = false
        let context = [
            "model=\(CaptureCalibration.model)",
            "ios=\(UIDevice.current.systemVersion)",
            "lens=\(device.localizedName)",
            "torch_device=\(torchDevice?.localizedName ?? "none")",
            "format=\(c.activeFormat.map { "\($0.width)x\($0.height)@\($0.frameRate)" } ?? "?")",
            String(format: "frame_us=%.3f", frameDurationNs / 1e3),
            String(format: "shutter_us=%.1f", device.exposureDuration.seconds * 1e6),
            String(format: "iso=%.0f", device.iso),
            "genlock=\(c.genlock.isEnabled)",
            "mode=\(settings.mode.rawValue)",
            "cycles=\(settings.cycles)",
            String(format: "on_ms=%.0f off_ms=%.0f", settings.onMs, settings.offMs),
        ]
        report("info", "Torch probe (\(settings.mode.rawValue)) started: \(settings.cycles) cycles, about "
            + String(format: "%.0f s", Double(settings.cycles) * (settings.onMs + settings.offMs) / 1e3 + 2)
            + (warnings.isEmpty ? "" : "; " + warnings.joined(separator: "; ")))

        // Appended and read on the session queue only.
        final class Commands: @unchecked Sendable {
            var list: [TorchProbeAnalysis.Command] = []
            var tai: [(before: Int64, after: Int64)] = []
        }
        let commands = Commands()
        @Sendable func toggle(_ edge: TorchProbeAnalysis.Edge) {
            guard let device = torchDevice else {
                return
            }
            do {
                try device.lockForConfiguration()
            } catch {
                return
            }
            defer { device.unlockForConfiguration() }
            let before = HostClock.nowNs()
            if edge == .on {
                do {
                    try device.setTorchModeOn(level: 1)
                } catch {
                    try? device.setTorchModeOn(level: max(0.1, AVCaptureDevice.maxAvailableTorchLevel))
                }
            } else {
                device.torchMode = .off
            }
            let after = HostClock.nowNs()
            commands.list.append(.init(edge: edge, beforeNs: before, afterNs: after))
            commands.tai.append((ClockMapping.at(hostNs: before).taiNs, ClockMapping.at(hostNs: after).taiNs))
        }

        // Dark baseline first, then the cycles. Each switch-on moves by a
        // golden-ratio fraction of a frame, so the switches cover the whole
        // frame phase evenly.
        lock.withLock { recording = true }
        let start = DispatchTime.now() + 1.0
        let period = (settings.onMs + settings.offMs) / 1e3
        for cycle in 0 ..< (switching ? settings.cycles : 0) {
            let phase = (Double(cycle) * 0.618_033_988_75).truncatingRemainder(dividingBy: 1) * frameDurationNs / 1e9
            let on = start + Double(cycle) * period + phase
            c.queue.asyncAfter(deadline: on) { toggle(.on) }
            c.queue.asyncAfter(deadline: on + settings.onMs / 1e3) { toggle(.off) }
        }
        let end = start + Double(settings.cycles) * period + 1.0
        c.queue.asyncAfter(deadline: end) { [self] in
            if c.exposureModeChanges == modeChanges, c.device === device {
                c.autoIso = autoIso
            }
            let (recorded, times) = lock.withLock {
                recording = false
                defer {
                    frames = []
                    frameTimes = []
                }
                return (frames, frameTimes)
            }
            let issued = commands.list
            let issuedTai = commands.tai
            let mode = settings.mode
            DispatchQueue.global(qos: .utility).async { [self] in
                let result = TorchProbeAnalysis.analyze(frames: recorded, commands: issued,
                                                        frameDurationNs: frameDurationNs)
                let (level, message) = mode == .watch
                    ? Self.watchSummary(recorded, times: times)
                    : Self.summary(result, frames: recorded.count, commands: issued.count)
                let folder = Self.write(result: result, frames: recorded, times: times, commands: issued,
                                        commandTai: issuedTai, context: context, summary: message)
                log.info("\(message, privacy: .public)")
                report(level, message + (folder.map { " (Documents/torch-probe/\($0))" } ?? ""))
                lock.withLock { running = false }
            }
        }
    }

    // MARK: Output

    static func watchSummary(_ frames: [TorchProbeAnalysis.Frame], times: [FrameTime]) -> (String, String) {
        let synced = times.filter(\.synced).count
        guard let levels = TorchProbeAnalysis.levels(frames) else {
            return ("warn", "Torch probe (watch): only \(frames.count) frames")
        }
        let level = levels.contrast >= TorchProbeAnalysis.minimumContrast ? "info" : "warn"
        return (level, String(format: "Torch probe (watch): %d frames (%d with a synced clock), luma %.0f to %.0f, %d lines",
                              frames.count, synced, levels.dark, levels.bright, frames.first?.profile.count ?? 0))
    }

    static func summary(_ result: TorchProbeAnalysis.Result?, frames: Int, commands: Int) -> (String, String) {
        guard let result else {
            return ("warn", "Torch probe: no usable contrast between torch off and on in \(frames) frames "
                + "(closer to the sheet, less room light, or a longer shutter)")
        }
        var parts = [String(format: "Torch probe: %d of %d switches seen, contrast %.0f (luma %.0f to %.0f), %d lines",
                            result.events.count, commands, result.levels.contrast,
                            result.levels.dark, result.levels.bright, result.rows)]
        if let reversed = result.reversed {
            parts.append(reversed ? "readout from the buffer's last line" : "readout from the buffer's first line")
        }
        for (name, fit) in [("on", result.on), ("off", result.off)] {
            guard let fit else {
                parts.append("\(name): too few edges to fit")
                continue
            }
            let edges = result.events.filter { $0.command.edge.rawValue == name }
            let widths = edges.map(\.crossing.widthRows).sorted()
            let width = widths.isEmpty ? 0 : widths[widths.count / 2]
            parts.append(String(
                format: "%@: line %.3f ± %.3f us (readout %.2f ms), latency - offset %.2f ms, jitter rms %.2f ms max %.2f ms, edge 10-90%% %.0f lines (%.2f ms), %d/%d fitted",
                name, fit.rowTimeNs / 1e3, fit.rowTimeErrorNs / 1e3, abs(fit.rowTimeNs) * Double(result.rows) / 1e6, fit.interceptNs / 1e6,
                fit.residualRmsNs / 1e6, fit.residualMaxNs / 1e6, width, width * abs(fit.rowTimeNs) / 1e6,
                fit.used, fit.total
            ))
        }
        if let ripple = result.ripple {
            parts.append(String(format: "ripple lit %.2f%% dark %.2f%%, %.0f crossings per lit frame",
                                ripple.litRelative * 100, ripple.darkRelative * 100, ripple.litCrossings))
        }
        return ("info", parts.joined(separator: "; "))
    }

    /// Writes events.csv, frames.csv and summary.txt; returns the folder name.
    private static func write(result: TorchProbeAnalysis.Result?, frames: [TorchProbeAnalysis.Frame],
                              times: [FrameTime], commands: [TorchProbeAnalysis.Command],
                              commandTai: [(before: Int64, after: Int64)], context: [String],
                              summary message: String) -> String?
    {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        let name = "\(formatter.string(from: Date()))-\(CaptureCalibration.model)"
        guard let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first else {
            return nil
        }
        let folder = documents.appendingPathComponent("torch-probe").appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            var summary = context.joined(separator: "\n") + "\n"
            summary += summaryLines(message)
            try summary.write(to: folder.appendingPathComponent("summary.txt"), atomically: true, encoding: .utf8)

            var events = "edge,before_ns,after_ns,frame_stamp_ns,command_after_stamp_us,row,width_rows,reversed\n"
            for event in result?.events ?? [] {
                events += String(format: "%@,%lld,%lld,%lld,%.3f,%.3f,%.3f,%d\n",
                                 event.command.edge.rawValue, event.command.beforeNs, event.command.afterNs,
                                 event.stampNs, event.commandAfterStampNs / 1e3, event.crossing.row,
                                 event.crossing.widthRows, event.crossing.reversed ? 1 : 0)
            }
            try events.write(to: folder.appendingPathComponent("events.csv"), atomically: true, encoding: .utf8)

            var commandLines = "edge,before_ns,after_ns,before_tai_ns,after_tai_ns\n"
            for (command, tai) in zip(commands, commandTai) {
                commandLines += "\(command.edge.rawValue),\(command.beforeNs),\(command.afterNs),\(tai.before),\(tai.after)\n"
            }
            try commandLines.write(to: folder.appendingPathComponent("commands.csv"), atomically: true, encoding: .utf8)

            // Per frame: host and TAI stamps and the whole profile (buffer
            // order), for the pair analysis off the phone.
            var frameLines = "stamp_ns,tai_ns,synced,lines,profile\n"
            for (frame, time) in zip(frames, times) {
                let values = frame.profile.map { String(format: "%.1f", $0) }
                frameLines += "\(frame.stampNs),\(time.taiNs),\(time.synced ? 1 : 0),\(frame.profile.count),"
                    + values.joined(separator: " ") + "\n"
            }
            try frameLines.write(to: folder.appendingPathComponent("frames.csv"), atomically: true, encoding: .utf8)
            return name
        } catch {
            log.error("Torch probe: cannot write results: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }

    private static func summaryLines(_ message: String) -> String {
        message
            .split(separator: ";")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .joined(separator: "\n") + "\n"
    }
}
