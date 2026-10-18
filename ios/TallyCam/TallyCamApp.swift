import SwiftUI

@main
struct TallyCamApp: App {
    init() {
        // Left by the operator/hub ownership of the shading values, now gone.
        for key in ["shadingOwnership", "shadingValues"] {
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}
