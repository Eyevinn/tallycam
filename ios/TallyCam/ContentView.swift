import SwiftUI

/// The shooting screen: full-bleed preview and tally, and the chrome on
/// one grid (docs/APPS.md, `Chrome`). Information in the middle, controls
/// at the sides, under the thumbs. Top: the status line and the camera
/// values row, with one slot under them for banners and toasts. Left and
/// right edges: a side control each (zoom, exposure, colour or focus,
/// chosen on it; a value in the values row puts its group on the right).
/// Bottom: tools left, the microphone meter right, and the live
/// button in the bottom-right corner (bottom centre in portrait).
/// Everything else is in Settings.
struct ContentView: View {
    /// Narrower than this (portrait iPhone), the camera's name gets a row of
    /// its own under the status line.
    private static let nameInlineWidth: CGFloat = 560

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
    /// A connect link for another hub, waiting for Join or Cancel. Not
    /// persisted: a restored screen never asks again.
    @State private var pendingJoin: ProvisioningLink?
    /// The join alert is up (after the Settings sheet has gone, if it was open).
    @State private var askingJoin = false
    /// What each side control runs; remembered (docs/UI_DESIGN.md, "Side
    /// control").
    @AppStorage("side.left") private var leftGroup = SideGroup.zoom
    @AppStorage("side.right") private var rightGroup = SideGroup.exposure
    #if DEBUG
        @State private var previewBackdrop = false
    #endif

    var body: some View {
        GeometryReader { geometry in
            let landscape = geometry.size.width > geometry.size.height
            let insets = geometry.safeAreaInsets
            ZStack {
                CameraStage(capture: capture, remote: remote, shading: remote.shading)
                #if DEBUG
                    if previewBackdrop {
                        // A stand-in picture, so the translucent chrome shows.
                        LinearGradient(colors: [Color(white: 0.55), Color(white: 0.3)],
                                       startPoint: .top, endPoint: .bottom)
                            .ignoresSafeArea()
                            .allowsHitTesting(false)
                    }
                #endif
                TallyOverlay(tally: remote.tally)
                if screenLocked {
                    ScreenLockShield()
                        .transition(.opacity)
                }
                let nameInline = geometry.size.width >= Self.nameInlineWidth
                chrome(landscape: landscape, nameInline: nameInline)
                    .padding(.top, Chrome.edge(insets.top))
                    .padding(.bottom, Chrome.edge(insets.bottom))
                    .padding(.leading, Chrome.edge(insets.leading))
                    .padding(.trailing, Chrome.edge(insets.trailing))
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
        .sheet(isPresented: $showSettings, onDismiss: {
            // A join asked for while Settings was open.
            askingJoin = pendingJoin != nil
        }) {
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
            #if DEBUG
                applyPreview()
            #endif
            await capture.start()
            #if DEBUG
                applyPreview()
            #endif
        }
        // Deep link from the control hub's QR code (Provisioning v0).
        .onOpenURL { url in
            provision(from: url)
        }
        .alert(pendingJoin?.joinTitle ?? "", isPresented: $askingJoin, presenting: pendingJoin) { link in
            Button("Join") {
                pendingJoin = nil
                remote.join(link, controller: controller)
            }
            .keyboardShortcut(.defaultAction)
            Button("Cancel", role: .cancel) {
                pendingJoin = nil
            }
        } message: { link in
            Text(link.joinMessage(live: controller.state != .idle))
        }
    }

    // MARK: Zones

    /// The whole chrome in one stack: the top rows, then the edge columns
    /// with the centre between them. While the screen is locked everything
    /// stays in place but only the slide to unlock takes touches.
    @ViewBuilder
    private func chrome(landscape: Bool, nameInline: Bool) -> some View {
        VStack(spacing: Chrome.zoneGap) {
            topRows(nameInline: nameInline)
                .allowsHitTesting(!screenLocked)
            if landscape {
                // Left: a side control over the tools. Centre: messages, the
                // slide to unlock, the meter. Right: a side control over the
                // live button. The bottom row is the live button's height, so
                // both edge columns centre on the same line.
                HStack(spacing: Chrome.gap) {
                    VStack(alignment: .leading, spacing: Chrome.zoneGap) {
                        sideColumn(.left)
                        tools
                            .frame(height: LiveButton.size, alignment: .bottom)
                    }
                    VStack(spacing: Chrome.zoneGap) {
                        messageSlot
                            .frame(maxHeight: .infinity, alignment: .top)
                        ZStack {
                            unlockSlider
                            audioMeter
                                .frame(maxWidth: .infinity, alignment: .trailing)
                        }
                        .frame(height: LiveButton.size, alignment: .bottom)
                    }
                    .frame(maxWidth: .infinity)
                    VStack(alignment: .trailing, spacing: Chrome.zoneGap) {
                        sideColumn(.right)
                        liveButton(hintAbove: true)
                    }
                }
            } else {
                // Messages in a band of their own (the side controls leave
                // little between them), then the side controls, mirror
                // images, their rockers level.
                messageSlot
                    .frame(height: Self.portraitMessageHeight, alignment: .top)
                HStack(spacing: Chrome.gap) {
                    sideColumn(.left)
                    Spacer(minLength: 0)
                    sideColumn(.right)
                }
                unlockSlider
                HStack(alignment: .bottom, spacing: Chrome.gap) {
                    tools
                    Spacer(minLength: 0)
                    audioMeter
                }
                liveButton(hintAbove: false)
            }
        }
        .animation(.easeInOut(duration: 0.2), value: leftGroup)
        .animation(.easeInOut(duration: 0.2), value: rightGroup)
        .animation(.easeInOut(duration: 0.2), value: screenLocked)
        .animation(.easeInOut(duration: 0.2), value: remote.banner)
    }

    /// The message band above the side controls in portrait: two lines of
    /// a banner.
    private static let portraitMessageHeight: CGFloat = 64

    /// Status line, then the camera values (the entry to exposure, colour,
    /// focus and Settings).
    private func topRows(nameInline: Bool) -> some View {
        VStack(spacing: Chrome.rowGap) {
            StatusStrip(controller: controller, remote: remote, capture: capture,
                        nameChip: nameInline ? nameChip : nil)
            {
                showSettings = true
            }
            if !nameInline {
                nameChip
            }
            CameraValuesRow(shading: remote.shading, capture: capture,
                            selected: [leftGroup.shading, rightGroup.shading].compactMap { $0 },
                            onSelect: selectFromValues)
            {
                showSettings = true
            }
        }
    }

    private var nameChip: TallyChip {
        TallyChip(tally: remote.tally, cameraName: controller.displayName)
    }

    /// A side control: the left or right edge's, the full height of its
    /// zone. Hidden (in place) while the screen is locked.
    private func sideColumn(_ side: Side) -> some View {
        SideControl(side: side, group: side == .left ? $leftGroup : $rightGroup,
                    other: side == .left ? $rightGroup : $leftGroup,
                    remote: remote, controls: capture.controls, shading: remote.shading)
            .opacity(screenLocked ? 0 : 1)
            .allowsHitTesting(!screenLocked)
    }

    /// A tap on a value in the values row: its group goes on the right side
    /// control, unless a side already shows it.
    private func selectFromValues(_ group: ShadingGroup) {
        let chosen = SideGroup(group)
        guard leftGroup != chosen, rightGroup != chosen else {
            return
        }
        rightGroup = chosen
    }

    /// Operator messages and toasts, centred under the values row, over the
    /// picture only.
    private var messageSlot: some View {
        VStack(spacing: Chrome.rowGap) {
            if let banner = remote.banner {
                Banner(text: banner)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
            if let toast {
                Toast(text: toast)
                    .transition(.opacity)
            }
        }
    }

    private func liveButton(hintAbove: Bool) -> some View {
        LiveButton(state: controller.state, hintAbove: hintAbove, onStart: start, onStop: { controller.stop() })
            .allowsHitTesting(!screenLocked)
    }

    @ViewBuilder
    private var unlockSlider: some View {
        if screenLocked {
            SlideToUnlock {
                withAnimation(.easeOut(duration: 0.2)) { screenLocked = false }
            }
            .transition(.opacity)
        }
    }

    /// Aids and screen lock; the QR scanner too while no hub is connected
    /// and not live.
    private var tools: some View {
        HStack(spacing: Chrome.gap) {
            AidsMenu()
            if !remote.connected, controller.state == .idle, !scanning {
                Button(action: startScan) {
                    ChromeButtonLabel(systemName: "qrcode.viewfinder")
                }
                .accessibilityLabel("Scan QR code")
            }
            Button {
                withAnimation(.easeOut(duration: 0.2)) {
                    screenLocked = true
                }
            } label: {
                ChromeButtonLabel(systemName: "lock")
            }
            .accessibilityLabel("Lock screen")
        }
        .allowsHitTesting(!screenLocked)
    }

    @ViewBuilder
    private var audioMeter: some View {
        if capture.hasAudio {
            AudioMeter(levels: capture.mic.levels)
        }
    }

    private var keepAwake: Bool {
        controller.state != .idle || remote.connected
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
            provision(from: link)
        }
    }

    /// A connect link (scanned or from the Camera app): joins at once when it
    /// is for the hub in use, else asks first (docs/UI_DESIGN.md, "Joining
    /// a hub").
    private func provision(from url: URL) {
        guard let link = remote.joinRequest(from: url) else {
            return
        }
        if remote.needsConfirmation(link) {
            pendingJoin = link
            if showSettings {
                // An alert does not show over the sheet: ask once it is gone.
                showSettings = false
            } else {
                askingJoin = true
            }
        } else {
            remote.join(link, controller: controller)
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

    #if DEBUG
        /// `-uiPreviewState YES`: made-up controls and values, tally, a banner
        /// and the exposure control open, to check the layout in the
        /// simulator (no camera). `-uiPreviewOrientation landscape` or
        /// `portrait` turns the screen.
        private func applyPreview() {
            let defaults = UserDefaults.standard
            if let orientation = defaults.string(forKey: "uiPreviewOrientation"),
               let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene
            {
                let mask: UIInterfaceOrientationMask = orientation == "portrait" ? .portrait : .landscapeRight
                scene.requestGeometryUpdate(.iOS(interfaceOrientations: mask)) { _ in }
            }
            guard defaults.bool(forKey: "uiPreviewState") else {
                return
            }
            func modes(_ values: [String]) -> ControlKind {
                .enumeration(values.map { ControlOption(value: .string($0), label: $0) })
            }
            let shading = remote.shading
            shading.update(descriptors: [
                ControlDescriptor(id: "exposure_mode", label: "Exposure", group: "exposure",
                                  kind: modes(["auto", "manual", "locked"])),
                ControlDescriptor(id: "iso", label: "ISO", group: "exposure",
                                  kind: .range(min: 25, max: 3200, step: 1, scale: "log")),
                ControlDescriptor(id: "ev_bias", label: "EV", group: "exposure", kind: .range(min: -2, max: 2, step: 0.1)),
                ControlDescriptor(id: "wb_mode", label: "White balance", group: "white_balance",
                                  kind: modes(["auto", "manual", "locked"])),
                ControlDescriptor(id: "wb_temperature", label: "K", group: "white_balance",
                                  kind: .range(min: 2000, max: 10000, step: 50)),
                ControlDescriptor(id: "wb_tint", label: "Tint", group: "white_balance",
                                  kind: .range(min: -150, max: 150, step: 1)),
                ControlDescriptor(id: "focus_mode", label: "Focus", group: "focus",
                                  kind: modes(["auto", "manual", "locked"])),
                ControlDescriptor(id: "focus_position", label: "Focus", group: "focus",
                                  kind: .range(min: 0, max: 1, step: 0.01)),
                ControlDescriptor(id: "lens", label: "Lens", group: "lens",
                                  kind: .enumeration([ControlOption(value: .string("ultra_wide"), label: "Ultra wide 0.5×"),
                                                      ControlOption(value: .string("wide"), label: "Wide 1×"),
                                                      ControlOption(value: .string("tele"), label: "Tele 4×")])),
            ])
            shading.update(values: [
                "exposure_mode": .string("auto"), "iso": .number(26), "shutter": .number(100), "ev_bias": .number(0.3),
                "wb_mode": .string("manual"), "wb_temperature": .number(5200), "wb_tint": .number(4),
                "focus_mode": .string("auto"), "focus_position": .number(0.42), "lens": .string("wide"),
            ])
            shading.preview(warning: "clipping")
            remote.preview(tally: .program, banner: "Wider please")
            connectCardDismissed = true
            previewBackdrop = true
            if let group = defaults.string(forKey: "uiPreviewGroup").flatMap(SideGroup.init(rawValue:)) {
                rightGroup = group
            }
            shading.hubSet(["wb_temperature"])
            if defaults.bool(forKey: "uiPreviewToast") {
                showToast("Lock the screen against stray touches: lock button, bottom left")
            }
        }
    #endif
}
