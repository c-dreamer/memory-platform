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
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> WKWebView {
        let view = WKWebView()
        view.navigationDelegate = context.coordinator
        let tokenPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/memory-platform/dashboard.token")
        let token = (try? String(contentsOf: tokenPath, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let url = URL(string: "http://127.0.0.1:8765/operations?token=\(token)")!
        context.coordinator.request = URLRequest(url: url)
        context.coordinator.loadWhenReady(in: view)
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        var request: URLRequest?
        private var attempts = 0

        func loadWhenReady(in view: WKWebView) {
            guard let request, attempts < 12 else { return }
            attempts += 1
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.75) { [weak view] in
                guard let view else { return }
                view.load(request)
            }
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation?, withError error: Error) {
            loadWhenReady(in: webView)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation?, withError error: Error) {
            loadWhenReady(in: webView)
        }
    }
}
