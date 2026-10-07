import SwiftUI
import WebKit
import UniformTypeIdentifiers

@main
struct PioneerCompanionApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var service = LocalService()

    var body: some Scene {
        WindowGroup {
            VStack(spacing: 0) {
                Text("OLC iPad preview · Build \(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?") · Direct-IP live status · Status-based waveform position")
                    .font(.caption)
                    .padding(6)
                    .frame(maxWidth: .infinity)
                    .background(Color(white: 0.15))
                if let session = service.session {
                    CompanionWebView(url: session.url, service: service).id(session.id)
                } else if let error = service.error {
                    ContentUnavailableView {
                        Label("Unable to open OneLibraryCompanion", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(error)
                    } actions: {
                        Button("Try again") { service.start() }
                    }
                } else {
                    ProgressView("Starting OneLibraryCompanion…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            // Keep the web viewport stable; the software keyboard overlays it.
            .ignoresSafeArea(.keyboard, edges: .bottom)
            .preferredColorScheme(.dark)
            .onAppear { service.start() }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { service.start() }
                if phase == .background { service.stop() }
            }
        }
    }
}

@MainActor
final class LocalService: ObservableObject {
    struct Session {
        let id = UUID()
        let url: URL
    }
    @Published var session: Session?
    @Published var error: String?
    private var usbFile: URL?
    private var usbRoots: [String: URL] = [:]
    private var usbTimer: Timer?
    private let usbBookmarkKey = "pc.local-usb.bookmarks"

    private func registerRoot(_ id: String, _ url: URL) -> String? {
        var buffer = [CChar](repeating: 0, count: 1024)
        let capacity = buffer.count
        let result = id.withCString { id in
            url.path.withCString { path in pc_local_usb(id, path, &buffer, capacity) }
        }
        return result == 0 ? nil : String(cString: buffer)
    }
    func selectUSBRoot(_ url: URL) -> String? {
        var saved = UserDefaults.standard.dictionary(forKey: usbBookmarkKey) as? [String: Data] ?? [:]
        if usbRoots.values.contains(where: { $0.standardizedFileURL == url.standardizedFileURL }) { return nil }
        guard saved.count < 3 else { return "Up to three local USBs are supported. Forget one before adding another." }
        guard url.startAccessingSecurityScopedResource() else { return "Choose the USB root folder in Files to grant access." }
        let id = UUID().uuidString
        do {
            let bookmark = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
            if let error = registerRoot(id, url) { url.stopAccessingSecurityScopedResource(); return error }
            saved[id] = bookmark
            UserDefaults.standard.set(saved, forKey: usbBookmarkKey)
            usbRoots[id] = url
            return nil
        } catch { url.stopAccessingSecurityScopedResource(); return error.localizedDescription }
    }
    func forgetUSBRoot(_ id: String) {
        _ = id.withCString { pc_local_usb($0, nil, nil, 0) }
        usbRoots.removeValue(forKey: id)?.stopAccessingSecurityScopedResource()
        var saved = UserDefaults.standard.dictionary(forKey: usbBookmarkKey) as? [String: Data] ?? [:]
        saved.removeValue(forKey: id)
        UserDefaults.standard.set(saved, forKey: usbBookmarkKey)
    }
    private func restoreUSBRoots() {
        guard session != nil else { return }
        var saved = UserDefaults.standard.dictionary(forKey: usbBookmarkKey) as? [String: Data] ?? [:]
        for (id, data) in saved {
            if let current = usbRoots[id], FileManager.default.fileExists(atPath: current.path) { continue }
            do {
                var stale = false
                let url = try URL(resolvingBookmarkData: data, options: [.withoutUI], relativeTo: nil, bookmarkDataIsStale: &stale)
                guard url.startAccessingSecurityScopedResource() else { continue }
                if registerRoot(id, url) != nil { url.stopAccessingSecurityScopedResource(); continue }
                usbRoots[id]?.stopAccessingSecurityScopedResource()
                usbRoots[id] = url
                if stale { saved[id] = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil) }
            } catch { /* Retry disconnected USB bookmarks while foregrounded. */ }
        }
        UserDefaults.standard.set(saved, forKey: usbBookmarkKey)
    }

    func selectUSBFile(_ url: URL) -> String? {
        guard url.startAccessingSecurityScopedResource() else {
            return "Cannot access this file. Choose it again from the USB in Files."
        }
        var buffer = [CChar](repeating: 0, count: 1024)
        let result = url.path.withCString { pc_usb_file($0, &buffer, buffer.count) }
        if result != 0 {
            url.stopAccessingSecurityScopedResource()
            return String(cString: buffer)
        }
        usbFile?.stopAccessingSecurityScopedResource()
        usbFile = url
        return nil
    }

    func start() {
        guard session == nil else { return }
        error = nil
        guard let root = Bundle.main.resourceURL?.appendingPathComponent("dist-ipad") else {
            error = "The app's interface files are missing."
            return
        }
        var buffer = [CChar](repeating: 0, count: 1024)
        let capacity = buffer.count
        let port = root.path.withCString { path in
            buffer.withUnsafeMutableBufferPointer { bytes in
                pc_start(path, bytes.baseAddress, capacity)
            }
        }
        guard port != 0 else {
            error = buffer.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
            return
        }
        session = Session(url: URL(string: "http://127.0.0.1:\(port)")!)
        UIApplication.shared.isIdleTimerDisabled = true
        restoreUSBRoots()
        usbTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.restoreUSBRoots() }
        }
    }

    func stop() {
        usbTimer?.invalidate()
        usbTimer = nil
        session = nil
        pc_stop()
        for url in usbRoots.values { url.stopAccessingSecurityScopedResource() }
        usbRoots.removeAll()
        usbFile?.stopAccessingSecurityScopedResource()
        usbFile = nil
        UIApplication.shared.isIdleTimerDisabled = false
    }
}

struct CompanionWebView: UIViewRepresentable {
    let url: URL
    let service: LocalService
    func makeCoordinator() -> Coordinator { Coordinator(origin: url, service: service) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.userContentController.add(context.coordinator, name: "usbProbe")
        configuration.userContentController.add(context.coordinator, name: "localUsb")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        context.coordinator.webView = view
        view.isOpaque = false
        view.backgroundColor = .black
        // Revalidate the entry page after app updates. Hashed assets retain their
        // normal cache behavior, and saved preferences/history remain intact.
        view.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData))
        return view
    }
    func updateUIView(_ view: WKWebView, context: Context) {}
    static func dismantleUIView(_ view: WKWebView, coordinator: Coordinator) {
        view.stopLoading()
        view.navigationDelegate = nil
        view.configuration.userContentController.removeScriptMessageHandler(forName: "usbProbe")
        view.configuration.userContentController.removeScriptMessageHandler(forName: "localUsb")
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler, UIDocumentPickerDelegate {
        let origin: URL
        let service: LocalService
        weak var webView: WKWebView?
        private var choosingUSBRoot = false
        init(origin: URL, service: LocalService) { self.origin = origin; self.service = service }
        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard ["usbProbe", "localUsb"].contains(message.name), message.frameInfo.isMainFrame,
                  message.frameInfo.securityOrigin.host == origin.host,
                  message.frameInfo.securityOrigin.port == origin.port,
                  message.frameInfo.securityOrigin.protocol == origin.scheme,
                  let controller = webView?.window?.rootViewController,
                  controller.presentedViewController == nil else { return }
            if message.name == "localUsb", message.body as? String == "forgetAll" {
                let saved = UserDefaults.standard.dictionary(forKey: "pc.local-usb.bookmarks") ?? [:]
                for id in saved.keys { service.forgetUSBRoot(id) }
                return
            }
            if message.name == "localUsb", let body = message.body as? [String: String], let id = body["forget"] {
                service.forgetUSBRoot(id); return
            }
            guard message.body as? String == "choose" else { return }
            choosingUSBRoot = message.name == "localUsb"
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: choosingUSBRoot ? [.folder] : [.wav], asCopy: false)
            picker.allowsMultipleSelection = false
            picker.delegate = self
            controller.present(picker, animated: true)
        }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            guard let url = urls.first else { return }
            if let message = choosingUSBRoot ? service.selectUSBRoot(url) : service.selectUSBFile(url) {
                let alert = UIAlertController(title: "Local USB", message: message, preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                // The picker dismisses itself before presenting an error.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                    self?.webView?.window?.rootViewController?.present(alert, animated: true)
                }
            }
        }
        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let destination = action.request.url else {
                decisionHandler(.cancel)
                return
            }
            let isLocal = destination.scheme == origin.scheme
                && destination.host == origin.host && destination.port == origin.port
            decisionHandler(isLocal ? .allow : .cancel)
        }
    }
}
