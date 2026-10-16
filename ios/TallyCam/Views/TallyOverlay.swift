import SwiftUI

/// Tally border and chip, and the operator message banner.
struct TallyOverlay: View {
    let tally: RemoteControl.Tally
    let banner: String?
    /// Space above the chip, so it sits below the status strip.
    var topPadding: CGFloat = 22

    var body: some View {
        ZStack(alignment: .top) {
            if let color {
                Rectangle()
                    .strokeBorder(color, lineWidth: 14)
                    .ignoresSafeArea()
            }
            VStack(spacing: 8) {
                if let color, let label {
                    Text(label)
                        .font(.headline.weight(.heavy))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                        .background(color, in: Capsule())
                }
                if let banner {
                    Text(banner)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(.black)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 18)
                        .padding(.vertical, 10)
                        .background(.yellow, in: RoundedRectangle(cornerRadius: 10))
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .padding(.top, topPadding)
        }
        .animation(.easeInOut(duration: 0.2), value: banner)
        .allowsHitTesting(false)
    }

    private var color: Color? {
        switch tally {
        case .off: nil
        case .preview: .green
        case .program: .red
        }
    }

    private var label: String? {
        switch tally {
        case .off: nil
        case .preview: "PREVIEW"
        case .program: "ON AIR"
        }
    }
}
