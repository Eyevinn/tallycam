import CoreGraphics
import Foundation

/// The operator's orientation setting, cycled by the overlay button and set
/// by the LCP control `orientation`.
enum OrientationMode: String, CaseIterable {
    /// Follow how the phone is held.
    case auto
    case landscape
    case portrait

    var next: OrientationMode {
        switch self {
        case .auto: .landscape
        case .landscape: .portrait
        case .portrait: .auto
        }
    }

    var label: String {
        switch self {
        case .auto: "Auto"
        case .landscape: "Landscape"
        case .portrait: "Portrait"
        }
    }

    /// SF Symbol for the overlay button.
    var symbol: String {
        switch self {
        case .auto: "rectangle.portrait.rotate"
        case .landscape: "rectangle"
        case .portrait: "rectangle.portrait"
        }
    }
}

/// How the phone (its user interface) is turned. "Home right" means the
/// bottom of the phone points right, which is the camera sensor's native
/// orientation.
enum DeviceSide: Equatable {
    case portrait
    case portraitUpsideDown
    case landscapeHomeRight
    case landscapeHomeLeft

    var isPortrait: Bool {
        self == .portrait || self == .portraitUpsideDown
    }
}

/// The orientation of the streamed frames: which way the capture connection
/// rotates the sensor image (clockwise, back camera).
enum StreamOrientation: Equatable {
    case landscapeHomeRight
    case portrait
    case landscapeHomeLeft
    case portraitUpsideDown


    /// `AVCaptureConnection.videoRotationAngle` for the back camera.
    var rotationAngle: CGFloat {
        switch self {
        case .landscapeHomeRight: 0
        case .portrait: 90
        case .landscapeHomeLeft: 180
        case .portraitUpsideDown: 270
        }
    }

    var isPortrait: Bool {
        self == .portrait || self == .portraitUpsideDown
    }

    /// Encoded size of a capture format of `landscape` size (as the sensor
    /// delivers it): unchanged in landscape, width and height swapped in
    /// portrait (1920x1080 becomes 1080x1920).
    func size(landscape: (width: Int32, height: Int32)) -> (width: Int32, height: Int32) {
        isPortrait ? (landscape.height, landscape.width) : landscape
    }

    /// The stream orientation for `mode` while the phone is held `side`.
    /// `lockedPortrait` (set while live) keeps the frame geometry of the
    /// running stream; only the upright side may still change, which flips
    /// the image without changing its size. `lastLandscape` picks the side
    /// for a landscape stream while the phone is held in portrait.
    static func resolve(mode: OrientationMode, side: DeviceSide, lastLandscape: DeviceSide,
                        lockedPortrait: Bool?) -> StreamOrientation
    {
        let portrait = lockedPortrait ?? {
            switch mode {
            case .auto: side.isPortrait
            case .landscape: false
            case .portrait: true
            }
        }()
        if portrait {
            return side == .portraitUpsideDown ? .portraitUpsideDown : .portrait
        }
        let landscapeSide = side.isPortrait ? lastLandscape : side
        return landscapeSide == .landscapeHomeLeft ? .landscapeHomeLeft : .landscapeHomeRight
    }

    /// Converts a normalized point in the streamed (rotated) frame to the
    /// device point-of-interest space, the unrotated sensor frame, for a
    /// clockwise rotation of `angle` degrees.
    static func devicePoint(x: Double, y: Double, angle: CGFloat) -> CGPoint {
        let x = min(max(x, 0), 1)
        let y = min(max(y, 0), 1)
        switch Int(angle.rounded()) {
        case 90: return CGPoint(x: y, y: 1 - x)
        case 180: return CGPoint(x: 1 - x, y: 1 - y)
        case 270: return CGPoint(x: 1 - y, y: x)
        default: return CGPoint(x: x, y: y)
        }
    }
}

/// The camera preview shows the streamed frame aspect-fitted in the view
/// (`resizeAspect`), so a tap maps to the stream frame through that rect.
enum PreviewGeometry {
    /// The rect the frame of `frameSize` occupies in a view of `viewSize`.
    static func videoRect(viewSize: CGSize, frameSize: CGSize) -> CGRect {
        guard viewSize.width > 0, viewSize.height > 0, frameSize.width > 0, frameSize.height > 0 else {
            return .zero
        }
        let scale = min(viewSize.width / frameSize.width, viewSize.height / frameSize.height)
        let size = CGSize(width: frameSize.width * scale, height: frameSize.height * scale)
        return CGRect(x: (viewSize.width - size.width) / 2, y: (viewSize.height - size.height) / 2,
                      width: size.width, height: size.height)
    }

    /// A tap at `point` in the view as a normalised (0...1) point of the
    /// streamed frame, or nil when it falls outside the picture.
    static func streamPoint(tap point: CGPoint, viewSize: CGSize, frameSize: CGSize) -> CGPoint? {
        let rect = videoRect(viewSize: viewSize, frameSize: frameSize)
        guard rect.width > 0, rect.contains(point) else {
            return nil
        }
        return CGPoint(x: (point.x - rect.minX) / rect.width, y: (point.y - rect.minY) / rect.height)
    }
}
