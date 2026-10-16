import SwiftUI

/// Shown once, centred, while no control hub is set up. Dismissible; the
/// same instructions stay in Settings.
struct ConnectCard: View {
    let onSettings: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "qrcode.viewfinder")
                .font(.system(size: 36, weight: .light))
            Text("Not connected")
                .font(.headline)
            Text("Scan the QR code in the control hub with the Camera app.")
                .font(.subheadline)
                .multilineTextAlignment(.center)
                .foregroundStyle(.white.opacity(0.8))
            HStack(spacing: 12) {
                Button("Enter manually", action: onSettings)
                    .buttonStyle(.bordered)
                    .foregroundStyle(.white)
                Button(action: onDismiss) {
                    Text("OK").frame(minWidth: 44)
                }
                .buttonStyle(.borderedProminent)
                .foregroundStyle(.black)
            }
            .tint(.white)
            .padding(.top, 4)
        }
        .foregroundStyle(.white)
        .padding(22)
        .frame(maxWidth: 320)
        .background(Color(white: 0.11), in: RoundedRectangle(cornerRadius: 18))
        .environment(\.colorScheme, .dark)
    }
}

/// A short message that goes away by itself (stream errors).
struct Toast: View {
    let text: String

    var body: some View {
        Label(text, systemImage: "exclamationmark.triangle.fill")
            .font(.footnote.weight(.medium))
            .foregroundStyle(.white)
            .lineLimit(3)
            .multilineTextAlignment(.leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.black.opacity(0.7), in: Capsule())
            .frame(maxWidth: 460)
            .allowsHitTesting(false)
    }
}
