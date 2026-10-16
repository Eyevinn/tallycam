import SwiftUI

/// A small vertical microphone meter: RMS as the bar, the peak as a line
/// falling back slowly, and a clip marker that holds red for a second after
/// a peak above -1 dBFS. Reads `MicLevels` about 15 times a second.
struct AudioMeter: View {
    let levels: MicLevels

    static let height: CGFloat = 84
    private static let floorDb: Float = -60
    private static let clipDb: Float = -1
    private static let clipHold: TimeInterval = 1
    /// Peak line fall-back per reading (about 20 dB per second).
    private static let peakFall: Float = 1.3

    @State private var rmsDb: Float = -90
    @State private var peakDb: Float = -90
    @State private var clipUntil = Date.distantPast

    var body: some View {
        let clipping = clipUntil > .now
        VStack(spacing: 3) {
            RoundedRectangle(cornerRadius: 1.5)
                .fill(clipping ? Color.red : .white.opacity(0.2))
                .frame(width: 12, height: 6)
            GeometryReader { geometry in
                let height = geometry.size.height
                ZStack(alignment: .bottom) {
                    Rectangle().fill(.white.opacity(0.12))
                    Rectangle()
                        .fill(Self.color(rmsDb))
                        .frame(height: height * Self.position(rmsDb))
                    Rectangle()
                        .fill(Self.color(peakDb))
                        .frame(height: 1.5)
                        .offset(y: -height * Self.position(peakDb))
                        .opacity(peakDb > Self.floorDb ? 1 : 0)
                }
            }
            .frame(width: 12)
            .clipShape(RoundedRectangle(cornerRadius: 1.5))
        }
        .frame(height: Self.height)
        .padding(.horizontal, 5)
        .padding(.vertical, 6)
        .background(.black.opacity(0.35), in: Capsule())
        .allowsHitTesting(false)
        .accessibilityLabel("Microphone level")
        .task {
            while !Task.isCancelled {
                read()
                try? await Task.sleep(for: .milliseconds(66))
            }
        }
    }

    private func read() {
        let (peak, rms) = levels.read()
        let peakNow = Self.db(peak)
        rmsDb = Self.db(rms)
        peakDb = max(peakNow, peakDb - Self.peakFall)
        if peakNow > Self.clipDb {
            clipUntil = Date(timeIntervalSinceNow: Self.clipHold)
        }
    }

    private static func db(_ linear: Float) -> Float {
        linear > 0 ? 20 * log10(linear) : -90
    }

    private static func position(_ db: Float) -> CGFloat {
        CGFloat(min(max((db - floorDb) / -floorDb, 0), 1))
    }

    private static func color(_ db: Float) -> Color {
        db > -6 ? .red : db > -18 ? .yellow : .green
    }
}
