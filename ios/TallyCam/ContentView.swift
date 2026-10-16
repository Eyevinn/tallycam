import SwiftUI

/// The shooting screen: full-bleed preview, tally, one status strip on top
/// and the live button away from where the hands hold the phone (right
/// edge, vertically centred in landscape; bottom centre in portrait).
/// Everything else is in Settings.
struct ContentView: View {
    /// Width of the tally border plus a margin: chrome stays clear of it.
    private static let tallyClearance: CGFloat = 22
    private static let stripHeight: CGFloat = 32

    @StateObject private var capture = Capture()
    @StateObject private var controller = StreamController()
    @StateObject private var remote = RemoteControl()

    @Environment(\.scenePhase) private var scenePhase

    @State private var showSettings = false
    @State private var connectCardDismissed = false
    @State private var toast: String?
    @State private var toastTask: Task<Void, Never>?

    var body: some View {
        GeometryReader { geometry in
            let landscape = geometry.size.width > geometry.size.height
            let insets = geometry.safeAreaInsets
            let top = Self.edgePadding(insets.top)
            ZStack {
                CameraStage(capture: capture)
                TallyOverlay(tally: remote.tally, banner: remote.banner, cameraName: controller.displayName,
                             topPadding: top + Self.stripHeight + 10)
                StatusStrip(controller: controller, remote: remote, capture: capture) {
                    showSettings = true
                }
                .padding(.top, top)
                .padding(.leading, Self.edgePadding(insets.leading))
                .padding(.trailing, Self.edgePadding(insets.trailing))
                .frame(maxHeight: .infinity, alignment: .top)
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
                if remote.isUnconfigured, !connectCardDismissed, !showSettings {
                    ConnectCard(onSettings: { showSettings = true },
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
            SettingsView(remote: remote, controller: controller, capture: capture)
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

    private var keepAwake: Bool {
        controller.state != .idle || remote.connected
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
        }
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
