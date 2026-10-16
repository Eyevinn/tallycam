import SwiftUI

/// The tally border around the picture: green on preview, red on air.
/// Edge only, so it never covers the picture's centre.
struct TallyOverlay: View {
    let tally: RemoteControl.Tally

    var body: some View {
        if let color = tally.color {
            Rectangle()
                .strokeBorder(color, lineWidth: 14)
                .ignoresSafeArea()
                .allowsHitTesting(false)
        }
    }
}

/// The camera's name, with PREVIEW or ON AIR in the tally colour: a compact
/// chip in the top row, so the person at the camera knows which camera this
/// is without it standing in the picture.
struct TallyChip: View {
    let tally: RemoteControl.Tally
    let cameraName: String

    var body: some View {
        HStack(spacing: 6) {
            if let label = tally.label {
                Text(label)
                    .font(.system(size: 15, weight: .heavy, design: .rounded))
            }
            Text(cameraName)
                .font(.system(size: 15, weight: .bold, design: .rounded))
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(tally.color ?? .black.opacity(0.55), in: Capsule())
        .accessibilityElement(children: .combine)
        .allowsHitTesting(false)
    }
}

/// A message from the hub to the person at the camera.
struct Banner: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.title3.weight(.semibold))
            .foregroundStyle(.black)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 18)
            .padding(.vertical, 10)
            .background(.yellow, in: RoundedRectangle(cornerRadius: 10))
            .allowsHitTesting(false)
    }
}

extension RemoteControl.Tally {
    var color: Color? {
        switch self {
        case .off: nil
        case .preview: .green
        case .program: .red
        }
    }

    var label: String? {
        switch self {
        case .off: nil
        case .preview: "PREVIEW"
        case .program: "ON AIR"
        }
    }
}
