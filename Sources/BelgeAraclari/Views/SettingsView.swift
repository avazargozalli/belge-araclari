import SwiftUI
import AppKit

struct SettingsView: View {
    @AppStorage(SettingsKey.outputMode) private var outputMode = 0
    @AppStorage(SettingsKey.outputFolder) private var outputFolder = ""
    @AppStorage(SettingsKey.useGhostscript) private var useGhostscript = true

    var body: some View {
        Form {
            Section("Kaydetme konumu") {
                Picker("Yeni dosyalar", selection: $outputMode) {
                    Text("Orijinal dosyanın yanına").tag(0)
                    Text("Seçtiğim klasöre").tag(1)
                }
                if outputMode == 1 {
                    HStack {
                        Text(outputFolder.isEmpty ? "Klasör seçilmedi" : (outputFolder as NSString).abbreviatingWithTildeInPath)
                            .foregroundStyle(outputFolder.isEmpty ? .secondary : .primary)
                            .lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button("Seç…", action: chooseFolder)
                    }
                }
                Text("Orijinal dosyalar hiçbir zaman değiştirilmez veya üzerine yazılmaz.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Section("PDF küçültme motoru") {
                Toggle("Ghostscript yüklüyse onu da dene", isOn: $useGhostscript)
                if let path = Compressor.ghostscriptPath() {
                    Label("Bulundu: \(path)", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green).font(.caption)
                } else {
                    Text("Ghostscript bulunamadı; yerleşik macOS motoru kullanılır. Daha güçlü sıkıştırma için isteğe bağlı olarak Homebrew ile kurabilirsiniz: brew install ghostscript")
                        .font(.caption).foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520, height: 360)
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Seç"
        if panel.runModal() == .OK, let url = panel.url { outputFolder = url.path }
    }
}

struct AboutView: View {
    var body: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 96, height: 96)
            VStack(spacing: 4) {
                Text("Belge Araçları").font(.title.weight(.bold))
                Text("Sürüm \(AppInfo.version)").foregroundStyle(.secondary)
            }
            Text("PDF küçültme, resimden PDF, birleştirme, bölme, sayfa düzenleme ve şifreleme. Tamamen yerel çalışır; hiçbir dosya veya veri internete gönderilmez.")
                .multilineTextAlignment(.center)
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(width: 320)
            Divider().frame(width: 220)
            VStack(spacing: 4) {
                Text("GELİŞTİREN").font(.caption2.weight(.semibold)).tracking(1.5).foregroundStyle(.secondary)
                Text("AZAR GOZALLI").font(.system(size: 17, weight: .bold)).tracking(4)
            }
            Text("Açık kaynak · MIT Lisansı · © 2026")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .padding(32)
        .frame(width: 400)
    }
}
