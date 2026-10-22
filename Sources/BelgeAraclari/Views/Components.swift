import SwiftUI
import AppKit
import PDFKit
import UniformTypeIdentifiers

// MARK: - Surfaces

extension Color {
    static let surface = Color(nsColor: .textBackgroundColor)
    static let canvas = Color(nsColor: .windowBackgroundColor)
    static let hairline = Color.primary.opacity(0.08)
}

struct CardStyle: ViewModifier {
    var padding: CGFloat = 20
    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color.surface))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.hairline))
            .shadow(color: .black.opacity(0.03), radius: 8, y: 2)
    }
}

extension View {
    func card(padding: CGFloat = 20) -> some View { modifier(CardStyle(padding: padding)) }
}

struct ToolIcon: View {
    let tool: Tool
    var size: CGFloat = 44

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
            .fill(tool.tint.gradient)
            .frame(width: size, height: size)
            .overlay(
                Image(systemName: tool.symbol)
                    .font(.system(size: size * 0.44, weight: .semibold))
                    .foregroundStyle(.white)
            )
            .shadow(color: tool.tint.opacity(size > 30 ? 0.25 : 0), radius: 6, y: 3)
    }
}

struct SectionTitle: View {
    let text: String
    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 11, weight: .semibold))
            .tracking(0.8)
            .foregroundStyle(.secondary)
    }
}

// MARK: - Tool page scaffold

struct ToolPage<Content: View, Bar: View>: View {
    let tool: Tool
    @ViewBuilder var content: () -> Content
    @ViewBuilder var bar: () -> Bar

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                HStack(spacing: 16) {
                    ToolIcon(tool: tool, size: 52)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(tool.title).font(.system(size: 26, weight: .bold))
                        Text(tool.subtitle).foregroundStyle(.secondary)
                    }
                    Spacer()
                    OfflineBadge()
                }
                content()
            }
            .padding(.horizontal, 36)
            .padding(.vertical, 30)
            .frame(maxWidth: 940)
            .frame(maxWidth: .infinity)
        }
        .background(Color.canvas)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                Divider()
                bar()
                    .padding(.horizontal, 36)
                    .padding(.vertical, 14)
                    .frame(maxWidth: 940)
                    .frame(maxWidth: .infinity)
            }
            .background(.bar)
        }
    }
}

struct OfflineBadge: View {
    var body: some View {
        Label("Çevrimdışı", systemImage: "lock.shield.fill")
            .font(.caption.weight(.semibold))
            .foregroundStyle(Color(red: 0.06, green: 0.55, blue: 0.4))
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(Capsule().fill(Color(red: 0.06, green: 0.62, blue: 0.47).opacity(0.12)))
            .help("Dosyalarınız bu Mac'ten hiçbir yere gönderilmez.")
    }
}

struct ActionBar: View {
    let job: Job
    let tool: Tool
    let title: String
    var info: String = ""
    var disabled: Bool
    let action: () -> Void

    var body: some View {
        HStack(spacing: 16) {
            if job.isRunning {
                ProgressView(value: job.progress)
                    .frame(width: 180)
                    .tint(tool.tint)
                Text(job.status)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            } else {
                Text(info).font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            Button(action: action) {
                HStack(spacing: 8) {
                    if job.isRunning {
                        ProgressView().controlSize(.small).tint(.white)
                    } else {
                        Image(systemName: tool.symbol)
                    }
                    Text(title).fontWeight(.semibold)
                }
                .frame(minWidth: 170)
                .padding(.vertical, 3)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .tint(tool.tint)
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(disabled || job.isRunning)
        }
    }
}

// MARK: - Drop zone

struct DropZone: View {
    let title: String
    let subtitle: String
    let types: [UTType]
    var multiple = true
    var compact = false
    var tint: Color = .accentColor
    let onFiles: ([URL]) -> Void

    @State private var targeted = false
    @State private var hovering = false

    var body: some View {
        Button(action: pick) {
            VStack(spacing: compact ? 6 : 12) {
                if !compact {
                    ZStack {
                        Circle().fill(tint.opacity(0.12)).frame(width: 68, height: 68)
                        Image(systemName: targeted ? "arrow.down.circle.fill" : "arrow.down.doc")
                            .font(.system(size: 28, weight: .medium))
                            .foregroundStyle(tint)
                    }
                }
                HStack(spacing: 8) {
                    if compact { Image(systemName: "plus.circle.fill").foregroundStyle(tint) }
                    Text(title).font(compact ? .callout.weight(.medium) : .title3.weight(.semibold))
                }
                Text(subtitle)
                    .font(compact ? .caption : .callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .frame(height: compact ? 82 : 230)
            .background(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(targeted ? tint.opacity(0.10) : (hovering ? tint.opacity(0.04) : Color.surface))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(targeted ? tint : Color.secondary.opacity(0.3),
                                  style: StrokeStyle(lineWidth: targeted ? 2 : 1.3, dash: [7, 5]))
            )
            .contentShape(RoundedRectangle(cornerRadius: 18))
            .animation(.easeOut(duration: 0.15), value: targeted)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .dropDestination(for: URL.self) { urls, _ in
            let accepted = filter(urls)
            guard !accepted.isEmpty else { return false }
            onFiles(accepted)
            return true
        } isTargeted: { targeted = $0 }
    }

    private func filter(_ urls: [URL]) -> [URL] {
        FileTypes.expand(urls).filter { url in
            guard let type = UTType(filenameExtension: url.pathExtension.lowercased()) else { return false }
            return types.contains { type.conforms(to: $0) }
        }
    }

    private func pick() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = types
        panel.allowsMultipleSelection = multiple
        panel.canChooseDirectories = false
        panel.prompt = "Seç"
        if panel.runModal() == .OK { onFiles(panel.urls) }
    }
}

// MARK: - File list

/// Loads a small preview for a PDF (first page) or image file in the background.
struct FileThumbnail: View {
    let url: URL
    var size = CGSize(width: 38, height: 48)
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.1))
            if let image {
                Image(nsImage: image).resizable().scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            } else {
                Image(systemName: FileTypes.isPDF(url) ? "doc.richtext" : "photo")
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: size.width, height: size.height)
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.hairline))
        .task(id: url) {
            let url = url
            image = await Task.detached(priority: .utility) { () -> NSImage? in
                if FileTypes.isPDF(url) {
                    // Keep the document alive while its page draws.
                    guard let doc = PDFDocument(url: url) else { return nil }
                    return doc.page(at: 0)?.thumbnail(of: CGSize(width: 120, height: 150), for: .cropBox)
                }
                guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
                      let cg = ImageCodec.loadOriented(source, index: 0, maxPixel: 160) else { return nil }
                return NSImage(cgImage: cg, size: .zero)
            }.value
        }
    }
}

struct FileListCard: View {
    let store: FileStore
    var tint: Color
    var reorderable = true
    var title = "Dosyalar"

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(title).font(.headline)
                Text("\(store.files.count) dosya · \(formatBytes(store.totalSize))")
                    .font(.callout).foregroundStyle(.secondary)
                Spacer()
                if reorderable && store.files.count > 1 {
                    Button("Ada göre sırala", systemImage: "arrow.up.arrow.down") { store.sortByName() }
                        .buttonStyle(.borderless)
                }
                Button("Temizle", systemImage: "trash") { withAnimation { store.clear() } }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            Divider()
            ForEach(Array(store.files.enumerated()), id: \.element.id) { index, item in
                FileRow(item: item, index: reorderable && store.files.count > 1 ? index + 1 : nil, tint: tint) {
                    withAnimation(.snappy) { store.remove(item) }
                }
                .draggable(item.id.uuidString) {
                    Label(item.name, systemImage: "doc").padding(8).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                }
                .dropDestination(for: String.self) { ids, _ in
                    guard reorderable, let raw = ids.first, let id = UUID(uuidString: raw) else { return false }
                    withAnimation(.snappy) { store.move(id, before: item.id) }
                    return true
                }
                if index < store.files.count - 1 { Divider().padding(.leading, 76) }
            }
        }
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color.surface))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.hairline))
    }
}

struct FileRow: View {
    let item: FileItem
    let index: Int?
    let tint: Color
    let onRemove: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 12) {
            if let index {
                Text("\(index)")
                    .font(.caption.monospacedDigit().weight(.semibold))
                    .foregroundStyle(tint)
                    .frame(width: 18)
            }
            FileThumbnail(url: item.url)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.name).lineLimit(1).truncationMode(.middle)
                HStack(spacing: 6) {
                    Text(formatBytes(item.size))
                    if FileTypes.isPDF(item.url) || item.pageCount > 1 {
                        Text("·")
                        Text("\(item.pageCount) sayfa")
                    }
                    if item.isEncrypted {
                        Label("Şifreli", systemImage: "lock.fill").foregroundStyle(.orange)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer()
            if index != nil {
                Image(systemName: "line.3.horizontal")
                    .foregroundStyle(.tertiary)
                    .opacity(hovering ? 1 : 0)
                    .help("Sürükleyerek sıralayın")
            }
            Button(action: onRemove) {
                Image(systemName: "xmark.circle.fill").font(.title3)
            }
            .buttonStyle(.borderless)
            .foregroundStyle(.secondary)
            .help("Listeden çıkar")
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 9)
        .background(hovering ? Color.primary.opacity(0.03) : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .contextMenu {
            Button("Finder'da göster") { NSWorkspace.shared.activateFileViewerSelecting([item.url]) }
            Button("Aç") { NSWorkspace.shared.open(item.url) }
            Divider()
            Button("Listeden çıkar", role: .destructive, action: onRemove)
        }
    }
}

// MARK: - Options

struct ChoiceCard: View {
    let title: String
    let subtitle: String
    let selected: Bool
    let tint: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(title).font(.headline)
                    Spacer()
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .foregroundStyle(selected ? tint : Color.secondary.opacity(0.5))
                        .font(.title3)
                }
                Text(subtitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .multilineTextAlignment(.leading)
            }
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(selected ? tint.opacity(0.08) : Color.canvas.opacity(0.5)))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(selected ? tint : Color.hairline, lineWidth: selected ? 2 : 1))
            .contentShape(RoundedRectangle(cornerRadius: 14))
        }
        .buttonStyle(.plain)
    }
}

struct OptionRow<Content: View>: View {
    let label: String
    @ViewBuilder var content: () -> Content

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).frame(width: 150, alignment: .leading).foregroundStyle(.secondary)
            content()
            Spacer(minLength: 0)
        }
    }
}

struct Banner: View {
    let text: String
    var symbol = "info.circle.fill"
    var color: Color = .blue

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol).foregroundStyle(color)
            Text(text).font(.callout).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(color.opacity(0.09)))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(color.opacity(0.2)))
    }
}

// MARK: - Results

struct ResultsView: View {
    let job: Job
    let tint: Color

    var body: some View {
        if let error = job.error {
            Banner(text: error, symbol: "exclamationmark.triangle.fill", color: .red)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
        }
        if !job.results.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Image(systemName: job.results.allSatisfy(\.success) ? "checkmark.seal.fill" : "info.circle.fill")
                        .foregroundStyle(job.results.allSatisfy(\.success) ? tint : .orange)
                    Text(job.results.allSatisfy(\.success) ? "Tamamlandı" : "İşlem bitti")
                        .font(.headline)
                    Spacer()
                    if let folder = job.results.compactMap(\.url).first?.deletingLastPathComponent() {
                        Button("Klasörü aç", systemImage: "folder") { NSWorkspace.shared.open(folder) }
                            .buttonStyle(.borderless)
                    }
                    Button("Kapat") { withAnimation { job.reset() } }
                        .buttonStyle(.borderless)
                        .foregroundStyle(.secondary)
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
                Divider()
                ForEach(job.results) { result in
                    ResultRow(result: result, tint: tint)
                    if result.id != job.results.last?.id { Divider().padding(.leading, 52) }
                }
            }
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color.surface))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(tint.opacity(0.35), lineWidth: 1.5))
            .transition(.opacity.combined(with: .move(edge: .bottom)))
        }
    }
}

struct ResultRow: View {
    let result: ResultItem
    let tint: Color

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: result.success ? (result.isFolder ? "folder.fill" : "doc.fill") : "exclamationmark.circle.fill")
                .font(.title3)
                .foregroundStyle(result.success ? tint : .orange)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 3) {
                Text(result.title).lineLimit(1).truncationMode(.middle).fontWeight(.medium)
                Text(result.detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            if let before = result.originalSize, let after = result.newSize {
                HStack(spacing: 6) {
                    Text(formatBytes(before)).foregroundStyle(.secondary).strikethrough()
                    Image(systemName: "arrow.right").font(.caption).foregroundStyle(.secondary)
                    Text(formatBytes(after)).fontWeight(.semibold).foregroundStyle(tint)
                }
                .font(.callout.monospacedDigit())
            }
            if let url = result.url {
                Button("Aç") { NSWorkspace.shared.open(url) }
                Button {
                    NSWorkspace.shared.activateFileViewerSelecting([url])
                } label: {
                    Image(systemName: "magnifyingglass")
                }
                .help("Finder'da göster")
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }
}
