import SwiftUI
import AppKit

struct OrganizeView: View {
    @Bindable var model: OrganizeModel
    private let tool = Tool.organize

    var body: some View {
        ToolPage(tool: tool) {
            if let error = model.loadError {
                Banner(text: error, symbol: "exclamationmark.triangle.fill", color: .red)
            }
            if let url = model.url {
                toolbar(url)
                grid
                ResultsView(job: model.job, tint: tool.tint)
            } else {
                DropZone(title: "Düzenlenecek PDF'i sürükleyin", subtitle: "Tek dosya\nveya seçmek için tıklayın",
                         types: FileTypes.pdf, multiple: false, tint: tool.tint) { model.load($0.first) }
            }
        } bar: {
            ActionBar(job: model.job, tool: tool, title: "Kaydet",
                      info: model.url == nil ? "PDF ekleyin" : "\(model.pages.count) sayfa · \(model.selection.count) seçili",
                      disabled: model.url == nil || model.pages.isEmpty || !model.isDirty, action: model.save)
        }
    }

    private func toolbar(_ url: URL) -> some View {
        HStack(spacing: 10) {
            FileThumbnail(url: url, size: CGSize(width: 30, height: 38))
            VStack(alignment: .leading, spacing: 2) {
                Text(url.lastPathComponent).fontWeight(.medium).lineLimit(1).truncationMode(.middle)
                Text("Tıklayarak seçin, ⌘ ile çoklu seçim · Sürükleyerek sıralayın")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Group {
                Button { model.rotate(by: -90) } label: { Image(systemName: "rotate.left") }
                    .help("Sola döndür")
                Button { model.rotate(by: 90) } label: { Image(systemName: "rotate.right") }
                    .help("Sağa döndür")
                Button(role: .destructive) { withAnimation(.snappy) { model.deleteSelected() } } label: { Image(systemName: "trash") }
                    .help("Seçili sayfaları sil")
            }
            .disabled(model.selection.isEmpty)
            Menu {
                Button("Tümünü seç") { model.selectAll() }.keyboardShortcut("a")
                Button("Sırayı ters çevir") { withAnimation(.snappy) { model.reverse() } }
                Button("Değişiklikleri geri al") { withAnimation(.snappy) { model.reset() } }
                Divider()
                Button("Başka PDF aç…") { pickAnother() }
                Button("Kapat") { model.close() }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
        .controlSize(.large)
        .card(padding: 14)
    }

    private var grid: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150, maximum: 190), spacing: 18)], spacing: 18) {
            ForEach(Array(model.pages.enumerated()), id: \.element.id) { position, item in
                PageTile(item: item, position: position + 1, selected: model.selection.contains(item.id),
                         thumbnailer: model.thumbnailer, tint: tool.tint)
                    .onTapGesture {
                        model.toggle(item, extend: NSEvent.modifierFlags.contains(.command) || NSEvent.modifierFlags.contains(.shift))
                    }
                    .draggable(item.id.uuidString)
                    .dropDestination(for: String.self) { ids, _ in
                        guard let raw = ids.first, let id = UUID(uuidString: raw) else { return false }
                        withAnimation(.snappy) { model.move(id, before: item.id) }
                        return true
                    }
                    .contextMenu {
                        Button("Sola döndür") { model.selection = [item.id]; model.rotate(by: -90) }
                        Button("Sağa döndür") { model.selection = [item.id]; model.rotate(by: 90) }
                        Divider()
                        Button("Sil", role: .destructive) { model.selection = [item.id]; withAnimation(.snappy) { model.deleteSelected() } }
                    }
            }
        }
        .padding(.vertical, 4)
    }

    private func pickAnother() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = FileTypes.pdf
        if panel.runModal() == .OK { model.load(panel.url) }
    }
}

struct PageTile: View {
    let item: PageItem
    let position: Int
    let selected: Bool
    let thumbnailer: PageThumbnailer?
    let tint: Color
    @State private var image: NSImage?

    var body: some View {
        VStack(spacing: 8) {
            ZStack {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(selected ? tint.opacity(0.10) : Color.surface)
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFit()
                        .shadow(color: .black.opacity(0.15), radius: 3, y: 1)
                        .rotationEffect(.degrees(Double(item.rotation)))
                        .padding(14)
                        .animation(.snappy, value: item.rotation)
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            .frame(height: 190)
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(selected ? tint : Color.hairline, lineWidth: selected ? 2.5 : 1)
            )
            .overlay(alignment: .topTrailing) {
                if selected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.title2)
                        .foregroundStyle(.white, tint)
                        .padding(8)
                }
            }
            HStack(spacing: 4) {
                Text("\(position)").fontWeight(.semibold)
                if item.index + 1 != position {
                    Text("(asıl \(item.index + 1))").foregroundStyle(.secondary)
                }
                if item.rotation != 0 {
                    Image(systemName: "rotate.right").foregroundStyle(tint)
                }
            }
            .font(.caption.monospacedDigit())
        }
        .contentShape(Rectangle())
        .task(id: item.index) {
            image = await thumbnailer?.thumbnail(item.index)
        }
    }
}
