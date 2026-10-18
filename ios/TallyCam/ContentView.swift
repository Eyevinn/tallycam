import SwiftUI

/// The shooting screen: full-bleed preview, tally, one status strip on top
/// and the live button away from where the hands hold the phone (right
/// edge, vertically centred in landscape; bottom centre in portrait).
/// Everything else is in Settings.
struct ContentView: View {
    /// Width of the tally border plus a margin: chrome stays clear of it.
    private static let tallyClearance: CGFloat = 22
    private static let stripHeight: CGFloat = 32
    /// The group chips' row under the status strip.

    @StateObject private var capture = Capture()
    @StateObject private var controller = StreamController()
    @StateObject private var remote = RemoteControl()

    @Environment(\.scenePhase) private var scenePhase

    @State private var showSettings = false
    @State private var connectCardDismissed = false
    @State private var toast: String?
    @State private var toastTask: Task<Void, Never>?
    /// Touches blocked except the slide to unlock; not persisted.
    @State private var screenLocked = false
    /// The in-app QR scanner is on (only while not live).
    @State private var scanning = false
    @State private var scanHint: String?

    var body: some View {
        GeometryReader { geometry in
            let landscape = geometry.size.width > geometry.size.height
            let insets = geometry.safeAreaInsets
            let top = Self.edgePadding(insets.top)
            ZStack {
                CameraStage(capture: capture, remote: remote, shading: remote.shading)
                TallyOverlay(tally: remote.tally)
                VStack(alignment: .leading, spacing: 6) {
                    StatusStrip(controller: controller, remote: remote, capture: capture) {
                        showSettings = true
                    }
                    // Chips left, name and tally right; on a narrow screen
                    // the name goes to its own row, still on the right.
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 6) {
                            groupChips
                            Spacer(minLength: 8)
                            tallyChip
                        }
                        VStack(alignment: .trailing, spacing: 6) {
                            HStack(spacing: 6) {
                                groupChips
                                Spacer(minLength: 0)
                            }
                            tallyChip
                        }
                    }
                    .padding(.horizontal, 4)
                    if let banner = remote.banner {
                        Banner(text: banner)
                            .frame(maxWidth: .infinity)
                            .padding(.horizontal, 20)
                            .padding(.top, 4)
                            .transition(.move(edge: .top).combined(with: .opacity))
                    }
                }
                .animation(.easeInOut(duration: 0.2), value: remote.banner)
                .padding(.top, top)
                .padding(.leading, Self.edgePadding(insets.leading))
                .padding(.trailing, Self.edgePadding(insets.trailing))
                .frame(maxHeight: .infinity, alignment: .top)
                if !screenLocked {
                    zoomControls(insets: insets)
                    operatorButtons(insets: insets)
                }
                if capture.hasAudio {
                    AudioMeter(levels: capture.mic.levels)
                        .padding(.trailing, Self.edgePadding(insets.trailing) + 4)
                        .padding(.bottom, Self.edgePadding(insets.bottom) + 4)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                }
                liveButton(landscape: landscape, insets: insets)
                if let toast {
                    Toast(text: toast)
                        .padding(.horizontal, 24)
                        .padding(.bottom, landscape
                            ? Self.edgePadding(insets.bottom) + 6
                            : 20 + LiveButton.size + 18)
                        .frame(maxHeight: .infinity, alignment: .bottom)
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if screenLocked {
                    ScreenLockLayer(bottomPadding: landscape
                        ? Self.edgePadding(insets.bottom) + 6
                        : 20 + LiveButton.size + 18) {
                        withAnimation(.easeOut(duration: 0.2)) { screenLocked = false }
                    }
                    .transition(.opacity)
                }
                if scanning {
                    ScanOverlay(hint: scanHint, onCancel: stopScan)
                        .transition(.opacity)
                }
                if remote.isUnconfigured, !connectCardDismissed, !showSettings, !screenLocked, !scanning {
                    ConnectCard(onScan: startScan,
                                onSettings: { showSettings = true },
                                onDismiss: { withAnimation { connectCardDismissed = true } })
                        .padding(24)
                        .transition(.opacity.combined(with: .scale(scale: 0.95)))
                }
            }
        }
        .background(.black)
        .statusBarHidden()
        .persistentSystemOverlays(.hidden)
        // While live, edge swipes need a second swipe (no accidental app switch).
        .defersSystemGestures(on: controller.state == .idle ? [] : .all)
        .sheet(isPresented: $showSettings) {
            SettingsView(remote: remote, controller: controller, capture: capture) {
                showSettings = false
                startScan()
            }
        }
        // A camera on a tripod must not lock itself: stay awake while live
        // or while a control server can reach it.
        .onChange(of: keepAwake, initial: true) { _, awake in
            UIApplication.shared.isIdleTimerDisabled = awake
        }
        .onChange(of: scenePhase) { old, phase in
            remote.logLifecycle("App \(old) -> \(phase), stream \(controller.state)")
            switch phase {
            case .background:
                controller.enteredBackground(capture: capture)
                remote.enteredBackground()
            case .active:
                remote.enteredForeground()
                controller.enteredForeground()
            default:
                break
            }
        }
        .onChange(of: controller.message) { _, message in
            if let message {
                showToast(message)
            }
        }
        .task {
            remote.start(capture: capture, controller: controller)
            await capture.start()
        }
        // Deep link from the control hub's QR code (Provisioning v0).
        .onOpenURL { url in
            remote.provision(from: url, controller: controller)
        }
    }

    private var groupChips: some View {
        HStack(spacing: 6) {
            GroupChips(remote: remote, shading: remote.shading, interactive: !screenLocked)
            ExposureWarningLabel(shading: remote.shading)
        }
    }

    private var tallyChip: some View {
        TallyChip(tally: remote.tally, cameraName: controller.displayName)
    }

    private var keepAwake: Bool {
        controller.state != .idle || remote.connected
    }

    /// Lens buttons above the zoom rocker, at the leading edge, vertically
    /// centred (away from the live button in both orientations).
    private func zoomControls(insets: EdgeInsets) -> some View {
        VStack(spacing: 10) {
            LensButtons(shading: remote.shading) { lens in
                capture.controls.apply(["lens": .string(lens)]) { _ in }
            }
            ZoomRocker { rate in
                if rate == 0 {
                    capture.controls.stopZoomRamp()
                } else {
                    capture.controls.rampZoom(rate: rate)
                }
            }
        }
        .padding(.leading, Self.edgePadding(insets.leading) + 4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    }

    /// Aids and screen lock, bottom leading; the QR scanner too while no
    /// hub is connected and not live.
    private func operatorButtons(insets: EdgeInsets) -> some View {
        HStack(spacing: 10) {
            AidsMenu()
            if !remote.connected, controller.state == .idle, !scanning {
                Button(action: startScan) {
                    ChromeButtonLabel(systemName: "qrcode.viewfinder")
                }
                .accessibilityLabel("Scan QR code")
            }
            Button {
                withAnimation(.easeOut(duration: 0.2)) { screenLocked = true }
            } label: {
                ChromeButtonLabel(systemName: "lock")
            }
            .accessibilityLabel("Lock screen")
        }
        .padding(.leading, Self.edgePadding(insets.leading) + 4)
        .padding(.bottom, Self.edgePadding(insets.bottom) + 4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
    }

    private func liveButton(landscape: Bool, insets: EdgeInsets) -> some View {
        LiveButton(state: controller.state, hintBeside: landscape,
                   onStart: start, onStop: { controller.stop() })
            .padding(.trailing, landscape ? Self.edgePadding(insets.trailing) + 4 : 0)
            .padding(.bottom, landscape ? 0 : 20)
            .frame(maxWidth: .infinity, maxHeight: .infinity,
                   alignment: landscape ? .trailing : .bottom)
    }

    private func start() {
        controller.goLive(capture: capture)
        // The same message again does not trigger onChange.
        if controller.state == .idle, let message = controller.message {
            showToast(message)
        } else if controller.state != .idle, !screenLocked {
            // Suggested, not forced: a camera on a tripod may need its controls.
            showToast("Lock the screen against stray touches: lock button, bottom left")
        }
    }

    /// Scans with the camera's own session until a TallyCam code is seen,
    /// then provisions as the deep link from the Camera app does. Not while
    /// live: the stream's session stays as it is.
    private func startScan() {
        guard controller.state == .idle else {
            showToast("Stop the stream to scan a QR code")
            return
        }
        scanHint = nil
        withAnimation(.easeOut(duration: 0.2)) { scanning = true }
        capture.scanQrCodes { code in
            guard let link = ProvisioningLink.scannedLink(code) else {
                scanHint = "Not a TallyCam code"
                return
            }
            stopScan()
            remote.provision(from: link, controller: controller)
        }
    }

    private func stopScan() {
        capture.scanQrCodes(onCode: nil)
        withAnimation(.easeIn(duration: 0.2)) { scanning = false }
    }

    private func showToast(_ text: String) {
        withAnimation(.easeOut(duration: 0.2)) { toast = text }
        toastTask?.cancel()
        toastTask = Task {
            try? await Task.sleep(for: .seconds(4))
            guard !Task.isCancelled else {
                return
            }
            withAnimation(.easeIn(duration: 0.3)) { toast = nil }
        }
    }

    /// Extra padding inside the safe area so chrome clears the tally border
    /// on edges without a safe-area inset (landscape top, for example).
    private static func edgePadding(_ inset: CGFloat) -> CGFloat {
        max(8, tallyClearance - inset)
    }
}
