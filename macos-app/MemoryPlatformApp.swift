import AppKit
import SwiftUI
import WebKit

@main
struct MemoryPlatformApp: App {
    init() {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        task.arguments = ["kickstart", "-k", "gui/\(getuid())/com.memory-platform.dashboard"]
        try? task.run()
    }

    var body: some Scene {
        WindowGroup("Memory Platform") {
            DashboardWebView()
                .frame(minWidth: 960, minHeight: 680)
        }
        .windowResizability(.contentSize)
    }
}

struct DashboardWebView: NSViewRepresentable {
    func makeNSView(context: Context) -> WKWebView {
        let view = WKWebView()
        let tokenPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/memory-platform/dashboard.token")
        let token = (try? String(contentsOf: tokenPath, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let url = URL(string: "http://127.0.0.1:8765/operations?token=\(token)")!
        view.load(URLRequest(url: url))
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {}
}
