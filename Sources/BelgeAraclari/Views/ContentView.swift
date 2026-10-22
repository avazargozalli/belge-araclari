import SwiftUI

struct ContentView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        @Bindable var state = state
        NavigationSplitView {
            List(selection: $state.selection) {
                Label("Ana sayfa", systemImage: "square.grid.2x2.fill")
                    .tag(SidebarItem.home)
                Section("Araçlar") {
                    ForEach(Tool.allCases) { tool in
                        Label {
                            Text(tool.title)
                        } icon: {
                            ToolIcon(tool: tool, size: 20)
                        }
                        .tag(SidebarItem.tool(tool))
                    }
                }
            }
            .listStyle(.sidebar)
            .safeAreaInset(edge: .bottom, spacing: 0) { SignatureView() }
            .navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 300)
        } detail: {
            detail
                .navigationTitle(title)
        }
    }

    private var title: String {
        if case .tool(let tool) = state.selection { return tool.title }
        return "Belge Araçları"
    }

    @ViewBuilder
    private var detail: some View {
        switch state.selection {
        case .tool(.compress): CompressView(model: state.compress)
        case .tool(.imagesToPDF): ImagesView(model: state.images)
        case .tool(.merge): MergeView(model: state.merge)
        case .tool(.split): SplitView(model: state.split)
        case .tool(.organize): OrganizeView(model: state.organize)
        case .tool(.pdfToImages): PDFToImagesView(model: state.pdfToImages)
        case .tool(.protect): ProtectView(model: state.protect)
        case .home, .none: HomeView()
        }
    }
}

// MARK: - Home

struct HomeView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 28) {
                VStack(alignment: .leading, spacing: 10) {
                    OfflineBadge()
                    Text("Belge araçları")
                        .font(.system(size: 36, weight: .bold))
                    Text("PDF dosyalarını küçültün, resimleri PDF'e çevirin, birleştirin ve düzenleyin. Tüm işlemler bu Mac'te yapılır; orijinal dosyalarınız asla değiştirilmez.")
                        .font(.title3)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                DropZone(title: "Dosyaları buraya bırakın",
                         subtitle: "PDF'ler küçültmeye, resimler PDF'e dönüştürmeye gider\nveya seçmek için tıklayın",
                         types: FileTypes.pdf + FileTypes.images, tint: Tool.compress.tint) { urls in
                    state.open(urls)
                }

                VStack(alignment: .leading, spacing: 14) {
                    SectionTitle(text: "Tüm araçlar")
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 330), spacing: 16)], spacing: 16) {
                        ForEach(Tool.allCases) { tool in
                            ToolCard(tool: tool) { state.selection = .tool(tool) }
                        }
                    }
                }

                HStack {
                    Spacer()
                    VStack(spacing: 4) {
                        Text("AZAR GOZALLI")
                            .font(.system(size: 12, weight: .semibold))
                            .tracking(3)
                        Text("tarafından geliştirildi · Açık kaynak")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                }
                .padding(.top, 8)
            }
            .padding(.horizontal, 44)
            .padding(.vertical, 36)
            .frame(maxWidth: 1120)
            .frame(maxWidth: .infinity)
        }
        .background(Color.canvas)
    }
}

struct ToolCard: View {
    let tool: Tool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 16) {
                ToolIcon(tool: tool, size: 50)
                VStack(alignment: .leading, spacing: 6) {
                    Text(tool.title).font(.title3.weight(.semibold))
                    Text(tool.subtitle)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .multilineTextAlignment(.leading)
                }
                Spacer(minLength: 0)
                Image(systemName: "arrow.up.right")
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(hovering ? tool.tint : Color.secondary.opacity(0.5))
            }
            .padding(20)
            .frame(maxWidth: .infinity, minHeight: 112, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color.surface))
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(hovering ? tool.tint.opacity(0.5) : Color.hairline, lineWidth: hovering ? 1.5 : 1)
            )
            .shadow(color: hovering ? tool.tint.opacity(0.15) : .black.opacity(0.03), radius: hovering ? 14 : 6, y: hovering ? 6 : 2)
            .scaleEffect(hovering ? 1.01 : 1)
            .contentShape(RoundedRectangle(cornerRadius: 18))
        }
        .buttonStyle(.plain)
        .onHover { hovering in withAnimation(.easeOut(duration: 0.15)) { self.hovering = hovering } }
    }
}

// MARK: - Signature

struct SignatureView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "lock.shield.fill")
                    .foregroundStyle(Tool.compress.tint)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Tamamen çevrimdışı").font(.caption.weight(.semibold))
                    Text("Dosyalar bu Mac'ten çıkmaz").font(.caption2).foregroundStyle(.secondary)
                }
            }
            Divider()
            VStack(alignment: .leading, spacing: 2) {
                Text("AZAR GOZALLI")
                    .font(.system(size: 11, weight: .bold))
                    .tracking(2.5)
                Text("Belge Araçları · v\(AppInfo.version)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

enum AppInfo {
    static var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "1.0"
    }
}
