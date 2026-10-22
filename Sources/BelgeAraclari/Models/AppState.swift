import SwiftUI
import Observation
import PDFKit
import ImageIO

enum Tool: String, CaseIterable, Identifiable, Hashable {
    case compress, imagesToPDF, merge, split, organize, pdfToImages, protect
    var id: String { rawValue }

    var title: String {
        switch self {
        case .compress: return "PDF Küçült"
        case .imagesToPDF: return "Resimden PDF'e"
        case .merge: return "PDF Birleştir"
        case .split: return "PDF Böl"
        case .organize: return "Sayfaları Düzenle"
        case .pdfToImages: return "PDF'ten Resme"
        case .protect: return "PDF Şifrele"
        }
    }

    var subtitle: String {
        switch self {
        case .compress: return "PDF dosyalarını makul kaliteyle sıkıştırın veya daha küçük hale getirin."
        case .imagesToPDF: return "JPG, PNG, WEBP, HEIC ve TIFF resimlerini tek PDF yapın."
        case .merge: return "Birden fazla PDF'i istediğiniz sırayla tek dosyada birleştirin."
        case .split: return "Her sayfayı ayrı dosya yapın veya belirli sayfaları çıkarın."
        case .organize: return "Sayfaları döndürün, silin ve sürükleyerek yeniden sıralayın."
        case .pdfToImages: return "PDF sayfalarını yüksek çözünürlüklü PNG veya JPEG olarak kaydedin."
        case .protect: return "PDF'e açılış şifresi ekleyin veya bildiğiniz şifreyi kaldırın."
        }
    }

    var symbol: String {
        switch self {
        case .compress: return "arrow.down.right.and.arrow.up.left"
        case .imagesToPDF: return "photo.on.rectangle.angled"
        case .merge: return "doc.on.doc"
        case .split: return "scissors"
        case .organize: return "square.grid.2x2"
        case .pdfToImages: return "photo.stack"
        case .protect: return "lock.doc"
        }
    }

    var tint: Color {
        switch self {
        case .compress: return Color(red: 0.06, green: 0.62, blue: 0.47)
        case .imagesToPDF: return Color(red: 0.15, green: 0.45, blue: 0.95)
        case .merge: return Color(red: 0.49, green: 0.33, blue: 0.93)
        case .split: return Color(red: 0.95, green: 0.51, blue: 0.13)
        case .organize: return Color(red: 0.05, green: 0.6, blue: 0.75)
        case .pdfToImages: return Color(red: 0.88, green: 0.25, blue: 0.5)
        case .protect: return Color(red: 0.3, green: 0.36, blue: 0.5)
        }
    }

    var shortcut: KeyEquivalent {
        KeyEquivalent(Character(String((Tool.allCases.firstIndex(of: self) ?? 0) + 1)))
    }

    var acceptsImages: Bool { self == .imagesToPDF }
}

enum SidebarItem: Hashable {
    case home
    case tool(Tool)
}

// MARK: - App state

@MainActor
@Observable
final class AppState {
    static let shared = AppState()

    var selection: SidebarItem? = .home

    let compress = CompressModel()
    let images = ImagesModel()
    let merge = MergeModel()
    let split = SplitModel()
    let organize = OrganizeModel()
    let pdfToImages = PDFToImagesModel()
    let protect = ProtectModel()

    /// Routes files opened from Finder / Dock / the home drop zone to the right tool.
    func open(_ urls: [URL]) {
        let files = FileTypes.expand(urls)
        let pdfs = files.filter(FileTypes.isPDF)
        let images = files.filter(FileTypes.isImage)

        if pdfs.isEmpty {
            guard !images.isEmpty else { return }
            self.images.store.add(images)
            selection = .tool(.imagesToPDF)
            return
        }

        if case .tool(let tool) = selection, tool != .imagesToPDF {
            add(pdfs, to: tool)
        } else {
            add(pdfs, to: .compress)
        }
    }

    private func add(_ pdfs: [URL], to tool: Tool) {
        switch tool {
        case .compress: compress.store.add(pdfs)
        case .merge: merge.store.add(pdfs)
        case .split: split.store.add(pdfs)
        case .organize: organize.load(pdfs.first)
        case .pdfToImages: pdfToImages.store.add(pdfs)
        case .protect: protect.store.add(pdfs)
        case .imagesToPDF: break
        }
        selection = .tool(tool)
    }
}

// MARK: - Files

struct FileItem: Identifiable, Hashable {
    let id = UUID()
    let url: URL
    let size: Int64
    let pageCount: Int
    let isEncrypted: Bool

    var name: String { url.lastPathComponent }

    static func make(_ url: URL) -> FileItem {
        var pages = 1
        var encrypted = false
        if FileTypes.isPDF(url) {
            if let doc = CGPDFDocument(url as CFURL) {
                pages = doc.numberOfPages
                encrypted = doc.isEncrypted
            } else {
                pages = 0
            }
        } else if let source = CGImageSourceCreateWithURL(url as CFURL, nil),
                  ["tif", "tiff"].contains(url.pathExtension.lowercased()) {
            pages = CGImageSourceGetCount(source)
        }
        return FileItem(url: url, size: fileSize(url), pageCount: pages, isEncrypted: encrypted)
    }
}

@MainActor
@Observable
final class FileStore {
    enum Kind { case pdf, image }

    let kind: Kind
    let single: Bool
    var files: [FileItem] = []

    init(kind: Kind, single: Bool = false) {
        self.kind = kind
        self.single = single
    }

    var totalSize: Int64 { files.reduce(0) { $0 + $1.size } }
    var totalPages: Int { files.reduce(0) { $0 + $1.pageCount } }

    func add(_ urls: [URL]) {
        let accepted = FileTypes.expand(urls).filter { kind == .pdf ? FileTypes.isPDF($0) : FileTypes.isImage($0) }
        let known = Set(files.map(\.url))
        let fresh = accepted.filter { !known.contains($0) }.map(FileItem.make)
        if single {
            if let first = fresh.first { files = [first] }
        } else {
            files += fresh
        }
    }

    func remove(_ item: FileItem) { files.removeAll { $0.id == item.id } }
    func clear() { files.removeAll() }

    func move(_ id: UUID, before target: UUID) {
        guard id != target, let from = files.firstIndex(where: { $0.id == id }) else { return }
        let item = files.remove(at: from)
        let to = files.firstIndex(where: { $0.id == target }) ?? files.count
        files.insert(item, at: from <= to ? min(to + 1, files.count) : to)
    }

    func sortByName() {
        files.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
}

// MARK: - Background jobs

struct ResultItem: Identifiable {
    let id = UUID()
    var title: String
    var detail: String
    var url: URL?
    var isFolder = false
    var success = true
    var originalSize: Int64?
    var newSize: Int64?
}

@MainActor
@Observable
final class Job {
    var isRunning = false
    var progress = 0.0
    var status = ""
    var results: [ResultItem] = []
    var error: String?

    func run(_ work: @escaping @Sendable (ProgressReporter) throws -> [ResultItem]) {
        guard !isRunning else { return }
        isRunning = true
        progress = 0
        status = "Hazırlanıyor…"
        results = []
        error = nil

        let reporter = ProgressReporter { [weak self] fraction, status in
            Task { @MainActor in
                guard let self, self.isRunning else { return }
                self.progress = fraction
                self.status = status
            }
        }
        Task { [weak self] in
            let outcome = await Task.detached(priority: .userInitiated) {
                Result { try work(reporter) }
            }.value
            guard let self else { return }
            switch outcome {
            case .success(let results): self.results = results
            case .failure(let error): self.error = error.localizedDescription
            }
            self.finish()
        }
    }

    func reset() {
        results = []
        error = nil
    }

    private func finish() {
        isRunning = false
        progress = 1
        status = ""
        if error == nil && results.contains(where: { $0.success && $0.url != nil }) {
            NSSound(named: "Glass")?.play()
        }
    }
}
