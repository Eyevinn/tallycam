@preconcurrency import AVFoundation

/// The in-app QR scanner: a metadata output on the camera's own session, so
/// scanning needs no second camera view and works on every iOS 17 device
/// (VisionKit's DataScannerViewController needs an A12). Added with the
/// camera and off until `setScanning(true)`; the app turns it on only
/// while not live.
final class QrScanner: NSObject, @unchecked Sendable, AVCaptureMetadataOutputObjectsDelegate {
    private let output = AVCaptureMetadataOutput()
    private let lock = NSLock()
    private var onCode: ((String) -> Void)?
    private var lastCode: String?

    /// Adds the output. Call inside the session's configuration.
    func attach(to session: AVCaptureSession) {
        guard session.canAddOutput(output) else {
            return
        }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.connection(with: .metadata)?.isEnabled = false
    }

    /// Call on the session queue. `onCode` gets each new code on the main
    /// queue (a code seen again in a row is not repeated).
    func setScanning(_ on: Bool, onCode: ((String) -> Void)?) {
        lock.withLock {
            self.onCode = on ? onCode : nil
            lastCode = nil
        }
        guard output.availableMetadataObjectTypes.contains(.qr) else {
            return
        }
        output.metadataObjectTypes = on ? [.qr] : []
        output.connection(with: .metadata)?.isEnabled = on
    }

    func metadataOutput(_: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject],
                        from _: AVCaptureConnection)
    {
        guard let code = objects.lazy.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first
        else {
            return
        }
        let handler: ((String) -> Void)? = lock.withLock {
            guard code != lastCode else {
                return nil
            }
            lastCode = code
            return onCode
        }
        handler?(code)
    }
}
