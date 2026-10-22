import SwiftUI
import Observation
import PDFKit

@MainActor
@Observable
final class CompressModel {
    let store = FileStore(kind: .pdf)
    let job = Job()
    var level: CompressionLevel = .balanced
    var scanMode = false

    func start() {
        let urls = store.files.map(\.url)
        let level = level, scanMode = scanMode
        let useGS = UserDefaults.standard.object(forKey: SettingsKey.useGhostscript) as? Bool ?? true
        job.run { progress in
            var results: [ResultItem] = []
            for (i, url) in urls.enumerated() {
                let prefix = urls.count > 1 ? "(\(i + 1)/\(urls.count)) " : ""
                progress(Double(i) / Double(urls.count), prefix + url.lastPathComponent)
                do {
                    let outcome = try Compressor.compress(url, level: level, scanMode: scanMode, useGhostscript: useGS) { step in
                        progress(Double(i) / Double(urls.count), prefix + step)
                    }
                    if let output = outcome.output {
                        let saved = 100 - Int((Double(outcome.newSize) / Double(max(outcome.originalSize, 1)) * 100).rounded())
                        results.append(ResultItem(title: output.lastPathComponent,
                                                  detail: "%\(saved) daha küçük · \(outcome.method)",
                                                  url: output, originalSize: outcome.originalSize, newSize: outcome.newSize))
                    } else {
                        results.append(ResultItem(title: url.lastPathComponent,
                                                  detail: "Bu dosya zaten optimize; daha fazla küçültülemedi. Orijinal korunuyor.",
                                                  url: nil, success: false, originalSize: outcome.originalSize))
                    }
                } catch {
                    results.append(ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false))
                }
            }
            return results
        }
    }
}

@MainActor
@Observable
final class ImagesModel {
    let store = FileStore(kind: .image)
    let job = Job()
    var options = ImagePDFOptions()
    var outputName = ""

    func start() {
        guard let first = store.files.first else { return }
        let urls = store.files.map(\.url)
        let options = options
        let name = outputName.trimmingCharacters(in: .whitespaces).isEmpty ? Output.baseName(first.url) : outputName
        let output = Output.uniqueFile(in: Output.directory(for: first.url), base: name, ext: "pdf")
        job.run { progress in
            let pages = try ImageToPDF.convert(urls, options: options, to: output, progress: progress)
            return [ResultItem(title: output.lastPathComponent,
                               detail: "\(pages) sayfa · \(formatBytes(fileSize(output)))", url: output)]
        }
    }
}

@MainActor
@Observable
final class MergeModel {
    let store = FileStore(kind: .pdf)
    let job = Job()
    var outputName = "Birleştirilmiş belge"

    func start() {
        guard let first = store.files.first else { return }
        let urls = store.files.map(\.url)
        let output = Output.uniqueFile(in: Output.directory(for: first.url), base: outputName, ext: "pdf")
        job.run { progress in
            let pages = try PDFEngine.merge(urls, to: output, progress: progress)
            return [ResultItem(title: output.lastPathComponent,
                               detail: "\(urls.count) dosya · \(pages) sayfa · \(formatBytes(fileSize(output)))", url: output)]
        }
    }
}

@MainActor
@Observable
final class SplitModel {
    enum Mode: String, CaseIterable, Identifiable {
        case eachPage, ranges, every
        var id: String { rawValue }
        var title: String {
            switch self {
            case .eachPage: return "Her sayfa ayrı"
            case .ranges: return "Sayfa seç"
            case .every: return "Her N sayfada"
            }
        }
    }

    let store = FileStore(kind: .pdf, single: true)
    let job = Job()
    var mode: Mode = .eachPage
    var ranges = ""
    var separateRanges = false
    var every = 2

    func start() {
        guard let file = store.files.first else { return }
        let splitMode: PDFEngine.SplitMode
        switch mode {
        case .eachPage: splitMode = .eachPage
        case .ranges: splitMode = .ranges(ranges, separateFiles: separateRanges)
        case .every: splitMode = .every(every)
        }
        job.run { progress in
            let (url, count, isFolder) = try PDFEngine.split(file.url, mode: splitMode, progress: progress)
            return [ResultItem(title: url.lastPathComponent,
                               detail: isFolder ? "\(count) PDF dosyası oluşturuldu" : formatBytes(fileSize(url)),
                               url: url, isFolder: isFolder)]
        }
    }
}

@MainActor
@Observable
final class PDFToImagesModel {
    let store = FileStore(kind: .pdf)
    let job = Job()
    var format: PDFEngine.ImageFormat = .png
    var dpi: Double = 150

    func start() {
        let urls = store.files.map(\.url)
        let format = format, dpi = CGFloat(dpi)
        job.run { progress in
            var results: [ResultItem] = []
            for (i, url) in urls.enumerated() {
                let sub = ProgressReporter { fraction, status in
                    progress((Double(i) + fraction) / Double(urls.count), status)
                }
                do {
                    let (folder, count) = try PDFEngine.exportImages(url, format: format, dpi: dpi, progress: sub)
                    results.append(ResultItem(title: folder.lastPathComponent, detail: "\(count) görsel · \(format.title)",
                                              url: folder, isFolder: true))
                } catch {
                    results.append(ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false))
                }
            }
            return results
        }
    }
}

@MainActor
@Observable
final class ProtectModel {
    enum Mode: String, CaseIterable, Identifiable {
        case encrypt, decrypt
        var id: String { rawValue }
        var title: String { self == .encrypt ? "Şifre ekle" : "Şifre kaldır" }
    }

    let store = FileStore(kind: .pdf)
    let job = Job()
    var mode: Mode = .encrypt
    var password = ""
    var confirm = ""
    var ownerPassword = ""

    var canStart: Bool {
        guard !store.files.isEmpty, !password.isEmpty else { return false }
        return mode == .decrypt || password == confirm
    }

    func start() {
        let urls = store.files.map(\.url)
        let mode = mode, password = password, owner = ownerPassword
        job.run { progress in
            var results: [ResultItem] = []
            for (i, url) in urls.enumerated() {
                progress(Double(i) / Double(urls.count), url.lastPathComponent)
                let suffix = mode == .encrypt ? "şifreli" : "şifresiz"
                let output = Output.uniqueFile(in: Output.directory(for: url), base: "\(Output.baseName(url)) - \(suffix)", ext: "pdf")
                do {
                    if mode == .encrypt {
                        try PDFEngine.encrypt(url, userPassword: password, ownerPassword: owner, to: output)
                    } else {
                        try PDFEngine.decrypt(url, password: password, to: output)
                    }
                    results.append(ResultItem(title: output.lastPathComponent,
                                              detail: mode == .encrypt ? "Açılış şifresi eklendi" : "Şifre kaldırıldı", url: output))
                } catch {
                    results.append(ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false))
                }
            }
            return results
        }
        self.password = ""
        self.confirm = ""
        self.ownerPassword = ""
    }
}

// MARK: - Page organizer

struct PageItem: Identifiable, Hashable {
    let id = UUID()
    let index: Int          // zero-based index in the source document
    var rotation: Int = 0   // extra rotation applied by the user
}

/// Renders page thumbnails off the main thread from a private copy of the document.
actor PageThumbnailer {
    private let document: PDFDocument?
    private var cache: [Int: NSImage] = [:]

    init(url: URL) { document = PDFDocument(url: url) }

    func thumbnail(_ index: Int) -> NSImage? {
        if let cached = cache[index] { return cached }
        guard let page = document?.page(at: index) else { return nil }
        let image = page.thumbnail(of: CGSize(width: 260, height: 260), for: .cropBox)
        cache[index] = image
        return image
    }
}

@MainActor
@Observable
final class OrganizeModel {
    let job = Job()
    private(set) var url: URL?
    private(set) var thumbnailer: PageThumbnailer?
    var pages: [PageItem] = []
    var selection: Set<UUID> = []
    var loadError: String?
    private var originalCount = 0

    var isDirty: Bool {
        pages.count != originalCount || pages.enumerated().contains { $0.element.index != $0.offset || $0.element.rotation != 0 }
    }

    func load(_ url: URL?) {
        guard let url else { return }
        loadError = nil
        job.reset()
        guard let doc = PDFDocument(url: url) else {
            loadError = AppError.cannotOpen(url.lastPathComponent).localizedDescription
            return
        }
        if doc.isLocked {
            loadError = AppError.locked(url.lastPathComponent).localizedDescription
            return
        }
        self.url = url
        thumbnailer = PageThumbnailer(url: url)
        originalCount = doc.pageCount
        pages = (0..<doc.pageCount).map { PageItem(index: $0) }
        selection = []
    }

    func close() {
        url = nil
        thumbnailer = nil
        pages = []
        selection = []
        job.reset()
    }

    func toggle(_ item: PageItem, extend: Bool) {
        if extend {
            if selection.contains(item.id) { selection.remove(item.id) } else { selection.insert(item.id) }
        } else {
            selection = selection == [item.id] ? [] : [item.id]
        }
    }

    func selectAll() { selection = Set(pages.map(\.id)) }

    func rotate(by degrees: Int) {
        for i in pages.indices where selection.contains(pages[i].id) {
            pages[i].rotation = PDFEngine.normalized(pages[i].rotation + degrees)
        }
    }

    func deleteSelected() {
        pages.removeAll { selection.contains($0.id) }
        selection = []
    }

    func reset() {
        pages = (0..<originalCount).map { PageItem(index: $0) }
        selection = []
    }

    func reverse() { pages.reverse() }

    func move(_ id: UUID, before target: UUID) {
        guard id != target, let from = pages.firstIndex(where: { $0.id == id }) else { return }
        let item = pages.remove(at: from)
        let to = pages.firstIndex(where: { $0.id == target }) ?? pages.count
        pages.insert(item, at: from <= to ? min(to + 1, pages.count) : to)
    }

    func save() {
        guard let url else { return }
        let order = pages.map(\.index), rotations = pages.map(\.rotation)
        let output = Output.uniqueFile(in: Output.directory(for: url), base: "\(Output.baseName(url)) - düzenlenmiş", ext: "pdf")
        job.run { progress in
            progress(0.3, "Sayfalar hazırlanıyor…")
            let source = try PDFEngine.open(url)
            let result = PDFEngine.makeDocument(from: source, pages: order, extraRotation: rotations)
            guard result.pageCount > 0 else { throw AppError.noPages }
            progress(0.8, "Kaydediliyor…")
            try PDFEngine.write(result, to: output)
            return [ResultItem(title: output.lastPathComponent,
                               detail: "\(result.pageCount) sayfa · \(formatBytes(fileSize(output)))", url: output)]
        }
    }
}
