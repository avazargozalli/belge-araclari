import SwiftUI
import AppKit

@main
struct BelgeAraclariApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var state = AppState.shared

    var body: some Scene {
        WindowGroup("Belge Araçları", id: "main") {
            ContentView()
                .environment(state)
                .frame(minWidth: 980, minHeight: 660)
        }
        .defaultSize(width: 1180, height: 780)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .appInfo) { AboutButton() }
            CommandGroup(replacing: .newItem) {
                Button("Dosya Aç…") { openPanel() }.keyboardShortcut("o")
            }
            CommandMenu("Araçlar") {
                Button("Ana sayfa") { state.selection = .home }.keyboardShortcut("0")
                Divider()
                ForEach(Tool.allCases) { tool in
                    Button(tool.title) { state.selection = .tool(tool) }
                        .keyboardShortcut(tool.shortcut)
                }
            }
        }

        Window("Belge Araçları Hakkında", id: "about") {
            AboutView()
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)

        Settings {
            SettingsView()
        }
    }

    private func openPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = FileTypes.pdf + FileTypes.images + FileTypes.udf
        panel.allowsMultipleSelection = true
        if panel.runModal() == .OK { state.open(panel.urls) }
    }
}

private struct AboutButton: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button("Belge Araçları Hakkında") { openWindow(id: "about") }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func application(_ application: NSApplication, open urls: [URL]) {
        Task { @MainActor in AppState.shared.open(urls) }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
