import SwiftUI

/// Full-bleed camera preview with tap to focus, or a quiet placeholder when
/// there is no camera (no access, simulator).
struct CameraStage: View {
    @ObservedObject var capture: Capture

    @State private var focusPoint: CGPoint?
    @State private var focusTask: Task<Void, Never>?

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                CameraPreview(session: capture.session, inputGeneration: capture.inputGeneration,
                              rotationAngle: capture.streamOrientation.rotationAngle)
                { side in
                    // Not during the view update that reported it.
                    Task { @MainActor in
                        capture.setDeviceSide(side)
                    }
                }
                if let error = capture.cameraError {
                    unavailable(error)
                }
                if let focusPoint {
                    FocusSquare()
                        .position(focusPoint)
                        .transition(.opacity)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture(coordinateSpace: .local) { location in
                focus(at: location, in: geometry.size)
            }
        }
        .ignoresSafeArea()
    }

    private func unavailable(_ message: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "video.slash")
                .font(.system(size: 34, weight: .light))
            Text(message)
                .font(.footnote)
                .multilineTextAlignment(.center)
        }
        .foregroundStyle(.white.opacity(0.45))
        .padding(40)
        .allowsHitTesting(false)
    }

    private func focus(at location: CGPoint, in size: CGSize) {
        guard capture.cameraError == nil else {
            return
        }
        let frame = capture.streamSize
        guard let point = PreviewGeometry.streamPoint(tap: location, viewSize: size,
                                                      frameSize: CGSize(width: Int(frame.width),
                                                                        height: Int(frame.height)))
        else {
            return
        }
        capture.focus(atStreamPoint: point)
        withAnimation(.easeOut(duration: 0.15)) {
            focusPoint = location
        }
        focusTask?.cancel()
        focusTask = Task {
            try? await Task.sleep(for: .seconds(1.2))
            guard !Task.isCancelled else {
                return
            }
            withAnimation(.easeIn(duration: 0.3)) {
                focusPoint = nil
            }
        }
    }
}

/// The brief yellow square where the user tapped to focus.
private struct FocusSquare: View {
    @State private var settled = false

    var body: some View {
        RoundedRectangle(cornerRadius: 2)
            .strokeBorder(.yellow, lineWidth: 1.5)
            .frame(width: 72, height: 72)
            .scaleEffect(settled ? 1 : 1.35)
            .onAppear {
                withAnimation(.spring(duration: 0.3)) {
                    settled = true
                }
            }
            .allowsHitTesting(false)
    }
}
