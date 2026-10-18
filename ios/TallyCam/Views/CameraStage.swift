import SwiftUI

/// Full-bleed camera preview with the operator's touch controls (tap to
/// focus, long press to lock focus, pinch to zoom) and the aids drawn over
/// the picture, or a quiet placeholder when there is no camera (no access,
/// simulator).
struct CameraStage: View {
    @ObservedObject var capture: Capture
    /// Tap and long press focus through the operator's local controls.
    let remote: RemoteControl
    @ObservedObject var shading: Shading

    private static let longPress: TimeInterval = 0.55
    /// A touch that moves further than this is not a tap or a long press.
    private static let slop: CGFloat = 12
    /// Pinch zoom is applied at most this often (the camera reports every
    /// change to the hub).
    private static let pinchInterval: TimeInterval = 0.05

    @AppStorage(Aid.grid) private var grid = false
    @AppStorage(Aid.horizon) private var horizon = false
    @AppStorage(Aid.safeAreas) private var safeAreas = false
    @AppStorage(Aid.centreCuts) private var centreCuts = false
    @AppStorage(Aid.peaking) private var peakingOn = false

    @StateObject private var peaking = FocusPeaking()

    @State private var focusPoint: CGPoint?
    @State private var focusTask: Task<Void, Never>?
    /// Where focus was locked by a long press; cleared when focus leaves
    /// `locked` or on the next tap.
    @State private var lockPoint: CGPoint?
    // The touch in progress (tap or long press).
    @State private var touchStart: Date?
    @State private var touchMoved = false
    @State private var touchPinched = false
    @State private var pressTask: Task<Void, Never>?
    @State private var longPressed = false
    // The pinch in progress.
    @State private var pinchBase: Double?
    @State private var pinchApplied = Date.distantPast

    var body: some View {
        GeometryReader { geometry in
            let size = geometry.size
            let rect = PreviewGeometry.videoRect(viewSize: size, frameSize: frameSize)
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
                } else if rect.width > 0 {
                    aids(in: rect)
                }
                if let lockPoint {
                    FocusSquare(label: "AF LOCK")
                        .position(lockPoint)
                        .transition(.opacity)
                }
                if let focusPoint {
                    FocusSquare()
                        .position(focusPoint)
                        .transition(.opacity)
                }
            }
            .contentShape(Rectangle())
            .gesture(touch(in: size).simultaneously(with: pinch))
        }
        .ignoresSafeArea()
        .onChange(of: peakingOn && capture.cameraError == nil, initial: true) { _, on in
            if on {
                peaking.start(outputs: capture.outputs)
            } else {
                peaking.stop()
            }
        }
        .onDisappear { peaking.stop() }
        .onChange(of: shading.mode(.focus)) { _, mode in
            if mode != "locked" {
                withAnimation(.easeIn(duration: 0.3)) { lockPoint = nil }
            }
        }
    }

    private var frameSize: CGSize {
        let frame = capture.streamSize
        return CGSize(width: Int(frame.width), height: Int(frame.height))
    }

    @ViewBuilder
    private func aids(in rect: CGRect) -> some View {
        if peakingOn, let mask = peaking.mask {
            Image(decorative: mask, scale: 1)
                .renderingMode(.template)
                .resizable()
                .interpolation(.medium)
                .foregroundStyle(Aid.peakingTint)
                .frame(width: rect.width, height: rect.height)
                .position(x: rect.midX, y: rect.midY)
                .allowsHitTesting(false)
        }
        if grid || safeAreas || centreCuts {
            FrameGuides(rect: rect, grid: grid, safeAreas: safeAreas, centreCuts: centreCuts)
        }
        if horizon {
            HorizonLevel(rect: rect)
        }
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

    // MARK: Gestures

    /// One finger: a tap focuses, holding still locks focus. Tracked by hand
    /// so it can run alongside the pinch.
    private func touch(in size: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .local)
            .onChanged { value in
                if touchStart == nil {
                    touchStart = .now
                    touchMoved = false
                    touchPinched = false
                    longPressed = false
                    let location = value.startLocation
                    pressTask = Task {
                        try? await Task.sleep(for: .seconds(Self.longPress))
                        guard !Task.isCancelled, !touchMoved, !touchPinched else {
                            return
                        }
                        longPressed = true
                        focusLock(at: location, in: size)
                    }
                }
                let moved = hypot(value.location.x - value.startLocation.x, value.location.y - value.startLocation.y)
                if moved > Self.slop {
                    touchMoved = true
                    pressTask?.cancel()
                }
            }
            .onEnded { value in
                pressTask?.cancel()
                let quick = touchStart.map { Date.now.timeIntervalSince($0) < Self.longPress } ?? false
                if quick, !touchMoved, !touchPinched, !longPressed {
                    focus(at: value.startLocation, in: size)
                }
                touchStart = nil
            }
    }

    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                touchPinched = true
                pressTask?.cancel()
                zoom(by: value.magnification, final: false)
            }
            .onEnded { value in
                zoom(by: value.magnification, final: true)
                pinchBase = nil
            }
    }

    /// Multiplies the zoom at the start of the pinch (a log scale), within
    /// the `zoom` control's range.
    private func zoom(by magnification: CGFloat, final: Bool) {
        guard let descriptor = shading.descriptor("zoom"), let low = descriptor.min, let high = descriptor.max,
              low > 0, high >= low
        else {
            return
        }
        let base = pinchBase ?? shading.values["zoom"]?.double ?? 1
        pinchBase = base
        guard final || Date.now.timeIntervalSince(pinchApplied) >= Self.pinchInterval else {
            return
        }
        pinchApplied = .now
        let zoom = min(max(base * Double(magnification), low), high)
        capture.controls.apply(["zoom": .number((zoom * 100).rounded() / 100)]) { _ in }
    }

    private func streamPoint(_ location: CGPoint, in size: CGSize) -> CGPoint? {
        guard capture.cameraError == nil else {
            return nil
        }
        return PreviewGeometry.streamPoint(tap: location, viewSize: size, frameSize: frameSize)
    }

    private func focus(at location: CGPoint, in size: CGSize) {
        guard let point = streamPoint(location, in: size) else {
            return
        }
        remote.localFocus(at: point)
        withAnimation(.easeOut(duration: 0.15)) {
            lockPoint = nil
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

    private func focusLock(at location: CGPoint, in size: CGSize) {
        guard let point = streamPoint(location, in: size) else {
            return
        }
        remote.localFocusLock(at: point)
        focusTask?.cancel()
        withAnimation(.easeOut(duration: 0.15)) {
            focusPoint = nil
            lockPoint = location
        }
    }
}

/// The yellow square where the user focused: brief after a tap, with
/// `AF LOCK` beneath it while focus is locked there.
private struct FocusSquare: View {
    var label: String?

    @State private var settled = false

    var body: some View {
        RoundedRectangle(cornerRadius: 2)
            .strokeBorder(.yellow, lineWidth: 1.5)
            .frame(width: 72, height: 72)
            .overlay(alignment: .bottom) {
                if let label {
                    Text(label)
                        .font(.system(size: 10, weight: .bold, design: .rounded))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 2)
                        .background(.yellow, in: RoundedRectangle(cornerRadius: 3))
                        .fixedSize()
                        .offset(y: 20)
                }
            }
            .scaleEffect(settled ? 1 : 1.35)
            .onAppear {
                withAnimation(.spring(duration: 0.3)) {
                    settled = true
                }
            }
            .allowsHitTesting(false)
    }
}
