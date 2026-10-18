import SwiftUI

/// Over the camera picture while the in-app QR scanner runs: a viewfinder
/// square, what to do, and Cancel.
struct ScanOverlay: View {
    /// Shown under the instructions (a code that is not TallyCam's).
    let hint: String?
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 18) {
            RoundedRectangle(cornerRadius: 22)
                .strokeBorder(.white.opacity(0.9), lineWidth: 3)
                .frame(width: 220, height: 220)
            VStack(spacing: 6) {
                Text("Point the camera at the QR code in the control hub")
                    .font(.subheadline.weight(.medium))
                    .multilineTextAlignment(.center)
                if let hint {
                    Text(hint)
                        .font(.footnote)
                        .foregroundStyle(.yellow)
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.black.opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
            Button("Cancel", action: onCancel)
                .buttonStyle(.bordered)
                .tint(.white)
        }
        .foregroundStyle(.white)
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.black.opacity(0.25))
        .environment(\.colorScheme, .dark)
    }
}
