import SwiftUI

/// The time of day from the phone's disciplined clock, "14:51:46:12" (frames
/// at the stream rate), the same on every camera (docs/UI_DESIGN.md, zone
/// 1): white while the clock is healthy, orange when not, dim with "--"
/// frames when it is not synchronised. A small badge at the bottom centre,
/// as on a camera monitor. Redrawn 10 times a second, its own text only.
struct Timecode: View {
    let fps: Int
    let health: ClockSummary.Health?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.1)) { _ in
            // The same TAI-to-UTC mapping the frames' timestamps use.
            let mapping = ClockMapping.at(hostNs: HostClock.nowNs())
            Text(UiRules.timecode(utcNs: mapping.utcNs, fps: fps, synced: mapping.synced))
                .font(.caption.monospacedDigit().weight(.semibold))
                .foregroundStyle(!mapping.synced ? .white.opacity(0.45) : health == .healthy ? .white : .orange)
                .shadow(color: .black.opacity(0.5), radius: 2)
                .lineLimit(1)
                .fixedSize()
                .padding(.horizontal, 8)
                .frame(height: SideMetrics.rowHeight)
                .sideSurface(Capsule())
                .allowsHitTesting(false)
                .accessibilityLabel("Time code")
        }
    }
}
