import Cocoa
import WebKit
import ServiceManagement

@main
struct OLCMain {
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        withExtendedLifetime(delegate) { app.run() }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate, WKDownloadDelegate {
    private var window: NSWindow!
    private var web: WKWebView!
    private var process: Process?
    private var log: FileHandle?
    private var timer: Timer?
    private var starting = true
    private var attempts = 0
    private var quitting = false
    private var checking = false
    private var activity: NSObjectProtocol?
    private let instance = UUID().uuidString
    private var origin = URL(string: "http://127.0.0.1:8787")!
    private var loginItem: NSMenuItem!

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let other = NSRunningApplication.runningApplications(withBundleIdentifier: Bundle.main.bundleIdentifier ?? "org.onelibrarycompanion.desktop").first(where: { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }) {
            other.activate(options: [.activateAllWindows])
            NSApp.terminate(nil)
            return
        }
        buildMenu()
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .default()
        web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = self
        web.uiDelegate = self
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 800), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "OneLibraryCompanion"
        window.contentView = web
        window.delegate = self
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 640, height: 440)
        window.center()
        window.setFrameAutosaveName("OLC.main")
        showWindow()
        web.loadHTMLString("<html style='background:#111;color:#eee;font:24px system-ui'><body><p>Starting OneLibraryCompanion…</p></body></html>", baseURL: nil)
        startHost()
    }
    private func buildMenu() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let app = NSMenu(title: "OneLibraryCompanion")
        app.addItem(withTitle: "About OneLibraryCompanion", action: #selector(about), keyEquivalent: "")
        app.addItem(.separator())
        app.addItem(withTitle: "Show OLC", action: #selector(showWindow), keyEquivalent: "0")
        loginItem = app.addItem(withTitle: "Open at Login", action: #selector(toggleLogin), keyEquivalent: "")
        loginItem.state = SMAppService.mainApp.status == .enabled ? .on : .off
        app.addItem(.separator())
        app.addItem(withTitle: "Hide OLC", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        app.addItem(withTitle: "Quit OLC", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = app
        main.addItem(appItem)
        let editItem = NSMenuItem()
        let edit = NSMenu(title: "Edit")
        for (title, action, key) in [("Undo", "undo:", "z"), ("Cut", "cut:", "x"), ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(withTitle: title, action: Selector(action), keyEquivalent: key)
        }
        editItem.submenu = edit
        main.addItem(editItem)
        let viewItem = NSMenuItem()
        let view = NSMenu(title: "View")
        view.addItem(withTitle: "Reload Interface", action: #selector(reload), keyEquivalent: "r")
        let full = view.addItem(withTitle: "Enter Full Screen", action: #selector(fullscreen), keyEquivalent: "f")
        full.keyEquivalentModifierMask = [.control, .command]
        viewItem.submenu = view
        main.addItem(viewItem)
        NSApp.mainMenu = main
    }
    private func startHost() {
        do {
            let resources = Bundle.main.resourceURL!
            let child = Process()
            child.executableURL = resources.appendingPathComponent("olc-host")
            var env = ProcessInfo.processInfo.environment
            env["OLC_UI_ROOT"] = resources.appendingPathComponent("dist").path
            env["OLC_INSTANCE"] = instance
            env["OLC_BIND"] = env["OLC_BIND"] ?? env["PIONEER_COMPANION_BIND"] ?? "0.0.0.0:8787"
            guard let port = env["OLC_BIND"]?.split(separator: ":").last.flatMap({ UInt16($0) }), port > 0 else {
                throw NSError(domain: "OLC", code: 1, userInfo: [NSLocalizedDescriptionKey: "OLC_BIND must specify a fixed port from 1 to 65535."])
            }
            origin = URL(string: "http://127.0.0.1:\(port)")!
            child.environment = env
            let data = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("OneLibraryCompanion")
            try FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
            let logURL = data.appendingPathComponent("host.log")
            FileManager.default.createFile(atPath: logURL.path, contents: nil)
            log = try FileHandle(forWritingTo: logURL)
            child.standardOutput = log
            child.standardError = log
            child.terminationHandler = { [weak self] _ in
                DispatchQueue.main.async {
                    guard let self, !self.quitting else { return }
                    self.timer?.invalidate()
                    let detail = (try? String(contentsOf: logURL, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
                    let reason = detail.flatMap { $0.isEmpty ? nil : String($0.suffix(2000)) } ?? "No additional details were recorded."
                    self.fail("The OneLibraryCompanion service stopped.\n\n\(reason)\n\nLog: \(logURL.path)")
                }
            }
            try child.run()
            process = child
            activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated], reason: "OLC serves live CDJ status to local and network clients")
            timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in self?.checkReady() }
        } catch { fail(error.localizedDescription) }
    }
    private func checkReady() {
        guard !checking else { return }
        attempts += 1
        if attempts > 120 { timer?.invalidate(); fail("The OLC service did not become ready. Quit and reopen OLC; check host.log in Application Support/OneLibraryCompanion."); return }
        checking = true
        var request = URLRequest(url: origin.appendingPathComponent("api/health"))
        request.timeoutInterval = 1
        URLSession.shared.dataTask(with: request) { [weak self] data, _, _ in
            DispatchQueue.main.async {
                guard let self else { return }
                self.checking = false
                guard self.starting, let data,
                      let health = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      health["instance"] as? String == self.instance else { return }
                self.starting = false
                self.timer?.invalidate()
                self.web.load(URLRequest(url: self.origin, cachePolicy: .reloadIgnoringLocalCacheData))
            }
        }.resume()
    }
    private func fail(_ message: String) {
        starting = false
        showWindow()
        let alert = NSAlert()
        alert.messageText = "Unable to run OneLibraryCompanion"
        alert.informativeText = message
        alert.addButton(withTitle: "Quit")
        alert.runModal()
        NSApp.terminate(nil)
    }
    @objc private func about() {
        NSApp.orderFrontStandardAboutPanel(options: [.applicationName: "OneLibraryCompanion", .applicationVersion: "0.1.2", .credits: NSAttributedString(string: "OLC · Shared CDJ companion\nGPL-3.0-only")])
    }
    @objc func showWindow() { window?.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true) }
    @objc private func reload() { if !starting { web.load(URLRequest(url: origin, cachePolicy: .reloadIgnoringLocalCacheData)) } }
    @objc private func fullscreen() { window.toggleFullScreen(nil) }
    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled { try SMAppService.mainApp.unregister() }
            else { try SMAppService.mainApp.register() }
            loginItem.state = SMAppService.mainApp.status == .enabled ? .on : .off
            if SMAppService.mainApp.status == .requiresApproval { SMAppService.openSystemSettingsLoginItems() }
        } catch {
            let alert = NSAlert(); alert.messageText = "Login startup could not be changed"; alert.informativeText = error.localizedDescription; alert.runModal()
        }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { sender.orderOut(nil); return false }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool { showWindow(); return true }
    func applicationWillTerminate(_ notification: Notification) {
        quitting = true
        timer?.invalidate()
        if let child = process, child.isRunning {
            child.terminate()
            let deadline = Date().addingTimeInterval(3)
            while child.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
            if child.isRunning { kill(child.processIdentifier, SIGKILL) }
        }
        if let activity { ProcessInfo.processInfo.endActivity(activity) }
        try? log?.close()
    }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        if url.scheme == "mailto" { NSWorkspace.shared.open(url); decisionHandler(.cancel); return }
        if action.shouldPerformDownload { decisionHandler(.download); return }
        let local = url.scheme == origin.scheme && url.host == origin.host && url.port == origin.port
        decisionHandler(local || url.scheme == "blob" || url.absoluteString == "about:blank" ? .allow : .cancel)
    }
    func webView(_ webView: WKWebView, decidePolicyFor response: WKNavigationResponse, decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void) {
        decisionHandler(response.canShowMIMEType ? .allow : .download)
    }
    func webView(_ webView: WKWebView, navigationAction: WKNavigationAction, didBecome download: WKDownload) { download.delegate = self }
    func webView(_ webView: WKWebView, navigationResponse: WKNavigationResponse, didBecome download: WKDownload) { download.delegate = self }
    func download(_ download: WKDownload, decideDestinationUsing response: URLResponse, suggestedFilename: String, completionHandler: @escaping (URL?) -> Void) {
        let panel = NSSavePanel(); panel.nameFieldStringValue = suggestedFilename
        panel.beginSheetModal(for: window) { result in completionHandler(result == .OK ? panel.url : nil) }
    }
    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping ([URL]?) -> Void) {
        let panel = NSOpenPanel(); panel.allowsMultipleSelection = parameters.allowsMultipleSelection; panel.canChooseDirectories = parameters.allowsDirectories
        panel.beginSheetModal(for: window) { result in completionHandler(result == .OK ? panel.urls : nil) }
    }
    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) {
        let alert = NSAlert(); alert.messageText = message; alert.addButton(withTitle: "OK"); alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { completionHandler($0 == .alertFirstButtonReturn) }
    }
    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void) {
        let alert = NSAlert(); alert.messageText = message; alert.beginSheetModal(for: window) { _ in completionHandler() }
    }
}
