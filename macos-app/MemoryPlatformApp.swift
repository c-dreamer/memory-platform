import AppKit
import SwiftUI
import WebKit

@main
struct MemoryPlatformApp: App {
    @NSApplicationDelegateAdaptor(AppLifecycle.self) private var lifecycle

    var body: some Scene {
        WindowGroup("Memory Platform") {
            DashboardWebView()
                .frame(minWidth: 960, minHeight: 680)
        }
        .windowResizability(.contentSize)
    }
}

final class AppLifecycle: NSObject, NSApplicationDelegate {
    private let labels = [
        "com.memory-platform.dashboard",
        "com.memory-platform.neon-sync",
        "com.memory-platform.neon-retry",
    ]

    func applicationDidFinishLaunching(_ notification: Notification) {
        for label in labels { bootstrap(label) }
        kickstart("com.memory-platform.dashboard")
        kickstart("com.memory-platform.neon-sync")
    }

    func applicationWillTerminate(_ notification: Notification) {
        for label in labels.reversed() { bootout(label) }
    }

    private func bootstrap(_ label: String) {
        let plist = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/\(label).plist")
        guard FileManager.default.fileExists(atPath: plist.path) else { return }
        run(["bootstrap", "gui/\(getuid())", plist.path])
    }

    private func kickstart(_ label: String) { run(["kickstart", "-k", "gui/\(getuid())/\(label)"]) }
    private func bootout(_ label: String) { run(["bootout", "gui/\(getuid())/\(label)"]) }

    private func run(_ arguments: [String]) {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        task.arguments = arguments
        try? task.run()
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
