import AVFoundation
import SwiftUI

/// Shows the camera rotated exactly like the streamed frames, so the preview
/// never shows a framing the stream does not have. While live with a locked
/// geometry, turning the phone the other way shows the stream sideways.
struct CameraPreview: UIViewRepresentable {
    let session: AVCaptureSession
    /// Changes when the camera input changes; the new preview connection
    /// needs the rotation again.
    var inputGeneration = 0
    /// The stream's capture rotation, applied to the preview as well.
    var rotationAngle: CGFloat = 0
    /// Called with how the user interface is turned.
    var onDeviceSide: (DeviceSide) -> Void = { _ in }

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        view.previewLayer.session = session
        view.previewLayer.videoGravity = .resizeAspect
        view.onDeviceSide = onDeviceSide
        view.rotationAngle = rotationAngle
        return view
    }

    func updateUIView(_ view: PreviewView, context: Context) {
        view.onDeviceSide = onDeviceSide
        view.rotationAngle = rotationAngle
        view.applyRotation()
    }

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
        var onDeviceSide: (DeviceSide) -> Void = { _ in }
        var rotationAngle: CGFloat = 0

        override func didMoveToWindow() {
            super.didMoveToWindow()
            setNeedsLayout()
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            if let side = Self.side(window?.windowScene?.interfaceOrientation) {
                onDeviceSide(side)
            }
            applyRotation()
        }

        func applyRotation() {
            guard let connection = previewLayer.connection,
                  connection.videoRotationAngle != rotationAngle,
                  connection.isVideoRotationAngleSupported(rotationAngle)
            else {
                return
            }
            connection.videoRotationAngle = rotationAngle
        }

        private static func side(_ orientation: UIInterfaceOrientation?) -> DeviceSide? {
            switch orientation {
            case .portrait: .portrait
            case .portraitUpsideDown: .portraitUpsideDown
            // UIInterfaceOrientation.landscapeRight: the home side is right.
            case .landscapeRight: .landscapeHomeRight
            case .landscapeLeft: .landscapeHomeLeft
            default: nil
            }
        }
    }
}
