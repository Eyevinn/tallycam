import CoreMotion
import SwiftUI
import UIKit

/// The operator's framing and focus aids, each a persisted toggle. Drawn
/// over the preview only, never into the stream (docs/SHADING.md).
/// Grid and horizon level are on until switched off, the others off.
enum Aid {
    static let grid = "aid.grid"
    static let horizon = "aid.horizon"
    static let safeAreas = "aid.safeAreas"
    static let centreCuts = "aid.centreCuts"
    static let peaking = "aid.peaking"
    /// Focus peaking edges: one colour, red, as most camera monitors.
    static let peakingTint = Color(red: 1, green: 0.15, blue: 0.1)
}

/// The small button that opens the aids menu, which also holds the torch
/// (where the lens has one; the hub can switch it too).
struct AidsMenu: View {
    let remote: RemoteControl
    @ObservedObject var shading: Shading

    @AppStorage(Aid.grid) private var grid = true
    @AppStorage(Aid.horizon) private var horizon = true
    @AppStorage(Aid.safeAreas) private var safeAreas = false
    @AppStorage(Aid.centreCuts) private var centreCuts = false
    @AppStorage(Aid.peaking) private var peaking = false

    var body: some View {
        Menu {
            Toggle("Grid (thirds)", isOn: $grid)
            Toggle("Horizon level", isOn: $horizon)
            Toggle("Safe areas (93% / 90%)", isOn: $safeAreas)
            Toggle("Centre cuts (4:3, 9:16)", isOn: $centreCuts)
            Section {
                Toggle("Focus peaking (sharp edges in red)", isOn: $peaking)
            }
            if shading.descriptor("torch") != nil {
                Section {
                    Toggle("Torch", isOn: Binding(
                        get: { shading.values["torch"]?.bool ?? false },
                        set: { remote.localSet(["torch": .bool($0)]) }
                    ))
                }
            }
        } label: {
            ChromeButtonLabel(systemName: "squareshape.split.3x3", active: anyOn)
        }
        .accessibilityLabel("Aids")
    }

    /// Any guide or peaking on.
    private var anyOn: Bool {
        grid || horizon || safeAreas || centreCuts || peaking || shading.values["torch"]?.bool == true
    }
}

/// Rule of thirds, safe areas and centre cuts inside the picture's rect.
struct FrameGuides: View {
    let rect: CGRect
    let grid: Bool
    let safeAreas: Bool
    let centreCuts: Bool

    /// Action safe and title safe, as a share of the frame.
    private static let actionSafe: CGFloat = 0.93
    private static let titleSafe: CGFloat = 0.90

    var body: some View {
        Canvas { context, _ in
            let line = StrokeStyle(lineWidth: 1)
            if grid {
                var path = Path()
                for i in 1 ... 2 {
                    let x = rect.minX + rect.width * CGFloat(i) / 3
                    let y = rect.minY + rect.height * CGFloat(i) / 3
                    path.move(to: CGPoint(x: x, y: rect.minY))
                    path.addLine(to: CGPoint(x: x, y: rect.maxY))
                    path.move(to: CGPoint(x: rect.minX, y: y))
                    path.addLine(to: CGPoint(x: rect.maxX, y: y))
                }
                context.stroke(path, with: .color(.white.opacity(0.4)), style: line)
            }
            if safeAreas {
                context.stroke(Path(Self.inset(rect, share: Self.actionSafe)),
                               with: .color(.white.opacity(0.5)), style: line)
                context.stroke(Path(Self.inset(rect, share: Self.titleSafe)),
                               with: .color(.white.opacity(0.35)), style: StrokeStyle(lineWidth: 1, dash: [6, 4]))
            }
            if centreCuts {
                for aspect in [4.0 / 3.0, 9.0 / 16.0] {
                    if let cut = Self.centreCut(rect, aspect: aspect) {
                        context.stroke(Path(cut), with: .color(.cyan.opacity(0.55)),
                                       style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                    }
                }
            }
        }
        .allowsHitTesting(false)
    }

    private static func inset(_ rect: CGRect, share: CGFloat) -> CGRect {
        rect.insetBy(dx: rect.width * (1 - share) / 2, dy: rect.height * (1 - share) / 2)
    }

    /// The largest rect of `aspect` (width / height) centred in `rect`, or
    /// nil when the frame already has about that aspect.
    private static func centreCut(_ rect: CGRect, aspect: CGFloat) -> CGRect? {
        guard rect.width > 0, rect.height > 0 else {
            return nil
        }
        let frameAspect = rect.width / rect.height
        guard abs(frameAspect / aspect - 1) > 0.02 else {
            return nil
        }
        let size = frameAspect > aspect
            ? CGSize(width: rect.height * aspect, height: rect.height)
            : CGSize(width: rect.width, height: rect.width / aspect)
        return CGRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2,
                      width: size.width, height: size.height)
    }
}

/// A horizon line from the gravity vector, green within a degree of level.
/// Shown only while the phone is held roughly upright (not lying flat, and
/// near one of the screen's axes).
struct HorizonLevel: View {
    let rect: CGRect

    @StateObject private var motion = HorizonMotion()

    var body: some View {
        ZStack {
            if let angle = motion.angle {
                let level = abs(angle) <= HorizonMotion.levelTolerance
                let width = min(rect.width, rect.height) * 0.6
                HStack(spacing: 0) {
                    Capsule().frame(width: width * 0.4, height: 2)
                    Spacer(minLength: 0)
                    Capsule().frame(width: width * 0.4, height: 2)
                }
                .frame(width: width)
                .overlay(Circle().strokeBorder(lineWidth: 1.5).frame(width: 10, height: 10))
                .foregroundStyle(level ? .green : .white.opacity(0.75))
                .rotationEffect(.radians(angle))
                .position(x: rect.midX, y: rect.midY)
            }
        }
        .allowsHitTesting(false)
        .onAppear { motion.start() }
        .onDisappear { motion.stop() }
    }
}

/// The roll of the phone against the screen's horizontal axis, from Core
/// Motion's gravity, on the main queue.
@MainActor
final class HorizonMotion: ObservableObject {
    static let levelTolerance = 1.0 * .pi / 180
    /// Beyond this roll the line is hidden (the phone is turning).
    private static let maxRoll = 30.0 * .pi / 180
    /// Gravity's share out of the screen plane beyond which the phone counts
    /// as lying flat.
    private static let maxFlat = 0.8

    /// Radians, clockwise on screen; nil while the phone is not upright.
    /// Published only when it moves by `step`: a phone on a tripod
    /// redraws nothing.
    @Published private(set) var angle: Double?

    /// Updates per second (smoothed), and the least change that redraws.
    private static let rate = 15.0
    private static let step = 0.05 * .pi / 180

    private let manager = CMMotionManager()
    private var smoothed: Double?

    func start() {
        guard manager.isDeviceMotionAvailable, !manager.isDeviceMotionActive else {
            return
        }
        manager.deviceMotionUpdateInterval = 1 / Self.rate
        manager.startDeviceMotionUpdates(to: .main) { [weak self] motion, _ in
            guard let gravity = motion?.gravity else {
                return
            }
            MainActor.assumeIsolated {
                self?.update(gravity)
            }
        }
    }

    func stop() {
        manager.stopDeviceMotionUpdates()
        angle = nil
        smoothed = nil
    }

    private func update(_ gravity: CMAcceleration) {
        let orientation = (UIApplication.shared.connectedScenes.first { $0 is UIWindowScene } as? UIWindowScene)?
            .interfaceOrientation ?? .portrait
        // Gravity in screen coordinates (x right, y down).
        let (x, y): (Double, Double) = switch orientation {
        case .landscapeRight: (-gravity.y, -gravity.x)
        case .landscapeLeft: (gravity.y, gravity.x)
        case .portraitUpsideDown: (-gravity.x, gravity.y)
        default: (gravity.x, -gravity.y)
        }
        // The world's horizontal on screen is perpendicular to gravity.
        let roll = atan2(-x, y)
        guard abs(gravity.z) < Self.maxFlat, abs(roll) < Self.maxRoll else {
            smoothed = nil
            if angle != nil {
                angle = nil
            }
            return
        }
        let value = smoothed.map { $0 + 0.45 * (roll - $0) } ?? roll
        smoothed = value
        if angle.map({ abs($0 - value) >= Self.step }) ?? true {
            angle = value
        }
    }
}
