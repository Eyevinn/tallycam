import SwiftUI

/// "TOO DARK" or "CLIPPING" from the image statistics, in amber.
struct ExposureWarningLabel: View {
    @ObservedObject var shading: Shading

    var body: some View {
        if let text {
            Text(text)
                .font(.system(size: 14, weight: .bold, design: .rounded))
                .foregroundStyle(.black)
                .padding(.horizontal, 7)
                .padding(.vertical, 4)
                .background(Color.orange, in: Capsule())
                .allowsHitTesting(false)
                .transition(.opacity)
        }
    }

    private var text: String? {
        switch shading.exposureWarning {
        case "too_dark": "TOO DARK"
        case "clipping": "CLIPPING"
        default: nil
        }
    }
}
