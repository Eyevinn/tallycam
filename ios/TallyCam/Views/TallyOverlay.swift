import SwiftUI

/// Tally border and chip, the camera's name, and the operator message
/// banner. The name is always shown, large, so the person at the camera
/// knows which camera they are; on air or on preview it sits in the tally
/// chip, in the border's colour.
struct TallyOverlay: View {
    let tally: RemoteControl.Tally
    let banner: String?
    let cameraName: String
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
                HStack(spacing: 10) {
                    if let label {
                        Text(label)
                            .font(.title3.weight(.heavy))
                        Text("·")
                    }
                    Text(cameraName)
                        .font(.title2.weight(.bold))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 18)
                .padding(.vertical, 8)
                .background(color ?? .black.opacity(0.45), in: Capsule())
                .padding(.horizontal, 24)
                .accessibilityElement(children: .combine)
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
