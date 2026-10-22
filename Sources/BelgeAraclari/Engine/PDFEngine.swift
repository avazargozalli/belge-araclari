import Foundation
import PDFKit

/// All page-level PDF operations. Everything runs locally through PDFKit / Core Graphics.
enum PDFEngine {
    static let creator = "Belge Araçları · Azar Gozalli"

    static func open(_ url: URL) throws -> PDFDocument {
        guard let doc = PDFDocument(url: url) else { throw AppError.cannotOpen(url.lastPathComponent) }
        if doc.isLocked { throw AppError.locked(url.lastPathComponent) }
        return doc
    }

    static func write(_ doc: PDFDocument, to url: URL, options: [PDFDocumentWriteOption: Any] = [:]) throws {
        var attributes = doc.documentAttributes ?? [:]
        attributes[PDFDocumentAttribute.creatorAttribute] = creator
        doc.documentAttributes = attributes
        guard doc.write(to: url, withOptions: options) else { throw AppError.cannotWrite(url.lastPathComponent) }
    }

    /// Copies the given pages (zero-based) into a fresh document.
    static func makeDocument(from source: PDFDocument, pages: [Int], extraRotation: [Int] = []) -> PDFDocument {
        let result = PDFDocument()
        for (i, index) in pages.enumerated() {
            guard let page = source.page(at: index)?.copy() as? PDFPage else { continue }
            if i < extraRotation.count, extraRotation[i] != 0 {
                page.rotation = normalized(page.rotation + extraRotation[i])
            }
            result.insert(page, at: result.pageCount)
        }
        return result
    }

    static func normalized(_ degrees: Int) -> Int { ((degrees % 360) + 360) % 360 }

    // MARK: Merge

    static func merge(_ urls: [URL], to output: URL, progress: ProgressReporter) throws -> Int {
        let result = PDFDocument()
        var sources: [PDFDocument] = []   // keep sources alive until the write finishes
        for (i, url) in urls.enumerated() {
            progress(Double(i) / Double(urls.count), "Ekleniyor: \(url.lastPathComponent)")
            let doc = try open(url)
            sources.append(doc)
            for index in 0..<doc.pageCount {
                if let page = doc.page(at: index)?.copy() as? PDFPage {
                    result.insert(page, at: result.pageCount)
                }
            }
        }
        guard result.pageCount > 0 else { throw AppError.noPages }
        progress(0.95, "Kaydediliyor…")
        try write(result, to: output)
        _ = sources
        return result.pageCount
    }

    // MARK: Split

    enum SplitMode {
        case eachPage
        case ranges(String, separateFiles: Bool)
        case every(Int)
    }

    /// Returns the written file (single result) or folder (several results) and the number of files.
    static func split(_ url: URL, mode: SplitMode, progress: ProgressReporter) throws -> (URL, Int, isFolder: Bool) {
        let doc = try open(url)
        let count = doc.pageCount
        guard count > 0 else { throw AppError.noPages }

        var groups: [[Int]]
        switch mode {
        case .eachPage:
            groups = (0..<count).map { [$0] }
        case .every(let n):
            let size = max(1, n)
            groups = stride(from: 0, to: count, by: size).map { Array($0..<min($0 + size, count)) }
        case .ranges(let text, let separate):
            let parsed = try parseRanges(text, pageCount: count)
            groups = separate ? parsed : [parsed.flatMap { $0 }]
        }

        let base = Output.baseName(url)
        let dir = Output.directory(for: url)

        if groups.count == 1 {
            let out = Output.uniqueFile(in: dir, base: "\(base) (\(label(groups[0])))", ext: "pdf")
            progress(0.5, "Kaydediliyor…")
            try write(makeDocument(from: doc, pages: groups[0]), to: out)
            return (out, 1, false)
        }

        let folder = try Output.uniqueFolder(in: dir, base: "\(base) - bölünmüş")
        let digits = String(count).count
        for (i, group) in groups.enumerated() {
            progress(Double(i) / Double(groups.count), "Dosya \(i + 1) / \(groups.count)")
            let name: String
            if group.count == 1 {
                name = "\(base) - sayfa \(String(format: "%0\(digits)d", group[0] + 1))"
            } else {
                name = "\(base) - \(label(group))"
            }
            try write(makeDocument(from: doc, pages: group), to: Output.uniqueFile(in: folder, base: name, ext: "pdf"))
        }
        return (folder, groups.count, true)
    }

    private static func label(_ pages: [Int]) -> String {
        guard let first = pages.first, let last = pages.last else { return "sayfalar" }
        if pages.count == 1 { return "sayfa \(first + 1)" }
        let contiguous = pages == Array(first...last)
        return contiguous ? "sayfa \(first + 1)-\(last + 1)" : "\(pages.count) sayfa"
    }

    /// Parses text like "1-3, 5, 8-" into zero-based page groups.
    static func parseRanges(_ text: String, pageCount: Int) throws -> [[Int]] {
        let normalized = text.replacingOccurrences(of: "–", with: "-").replacingOccurrences(of: ";", with: ",")
        var groups: [[Int]] = []
        for raw in normalized.split(separator: ",") {
            let token = raw.trimmingCharacters(in: .whitespaces)
            if token.isEmpty { continue }
            if token.contains("-") {
                let parts = token.split(separator: "-", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
                guard parts.count == 2 else { throw AppError.invalidRange(token) }
                let start = parts[0].isEmpty ? 1 : Int(parts[0])
                let end = parts[1].isEmpty ? pageCount : Int(parts[1])
                guard let s = start, let e = end, s >= 1, e <= pageCount, s <= e else { throw AppError.invalidRange(token) }
                groups.append(Array((s - 1)...(e - 1)))
            } else {
                guard let n = Int(token), n >= 1, n <= pageCount else { throw AppError.invalidRange(token) }
                groups.append([n - 1])
            }
        }
        guard !groups.isEmpty else { throw AppError.invalidRange(text) }
        return groups
    }

    // MARK: Rendering

    /// Renders the visible (crop box) area of a page, rotation applied, onto a white bitmap.
    static func renderPage(_ page: PDFPage, dpi: CGFloat) -> CGImage? {
        let box = page.bounds(for: .cropBox)
        let quarterTurn = normalized(page.rotation) % 180 != 0
        let size = quarterTurn ? CGSize(width: box.height, height: box.width) : box.size
        var scale = dpi / 72
        let maxPixels: CGFloat = 14_000
        scale = min(scale, maxPixels / max(size.width, size.height, 1))
        let width = max(1, Int((size.width * scale).rounded()))
        let height = max(1, Int((size.height * scale).rounded()))
        guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
        ctx.interpolationQuality = .high
        ctx.scaleBy(x: CGFloat(width) / size.width, y: CGFloat(height) / size.height)
        page.transform(ctx, for: .cropBox)
        page.draw(with: .cropBox, to: ctx)
        return ctx.makeImage()
    }

    /// Visible page size in points with rotation applied.
    static func displaySize(_ page: PDFPage) -> CGSize {
        let box = page.bounds(for: .cropBox)
        return normalized(page.rotation) % 180 != 0 ? CGSize(width: box.height, height: box.width) : box.size
    }

    // MARK: PDF → images

    enum ImageFormat: String, CaseIterable, Identifiable {
        case png, jpeg
        var id: String { rawValue }
        var title: String { self == .png ? "PNG" : "JPEG" }
        var ext: String { self == .png ? "png" : "jpg" }
    }

    static func exportImages(_ url: URL, format: ImageFormat, dpi: CGFloat, progress: ProgressReporter) throws -> (URL, Int) {
        let doc = try open(url)
        guard doc.pageCount > 0 else { throw AppError.noPages }
        let base = Output.baseName(url)
        let folder = try Output.uniqueFolder(in: Output.directory(for: url), base: "\(base) - görseller")
        let digits = String(doc.pageCount).count
        for index in 0..<doc.pageCount {
            progress(Double(index) / Double(doc.pageCount), "\(url.lastPathComponent): sayfa \(index + 1) / \(doc.pageCount)")
            try autoreleasepool {
                guard let page = doc.page(at: index), let image = renderPage(page, dpi: dpi) else { return }
                let data = format == .png ? ImageCodec.pngData(image) : ImageCodec.jpegData(image, quality: 0.9)
                guard let data else { throw AppError.cannotWrite("sayfa \(index + 1)") }
                let name = "\(base) - sayfa \(String(format: "%0\(digits)d", index + 1))"
                try data.write(to: Output.uniqueFile(in: folder, base: name, ext: format.ext))
            }
        }
        return (folder, doc.pageCount)
    }

    // MARK: Passwords

    static func encrypt(_ url: URL, userPassword: String, ownerPassword: String, to output: URL) throws {
        let doc = try open(url)
        let owner = ownerPassword.isEmpty ? userPassword : ownerPassword
        try write(doc, to: output, options: [.userPasswordOption: userPassword, .ownerPasswordOption: owner])
    }

    static func decrypt(_ url: URL, password: String, to output: URL) throws {
        guard let doc = PDFDocument(url: url) else { throw AppError.cannotOpen(url.lastPathComponent) }
        guard doc.isEncrypted else { throw AppError.message("\"\(url.lastPathComponent)\" zaten şifresiz.") }
        if doc.isLocked && !doc.unlock(withPassword: password) {
            throw AppError.wrongPassword(url.lastPathComponent)
        }
        // Copying pages into a new document drops the encryption dictionary.
        let plain = makeDocument(from: doc, pages: Array(0..<doc.pageCount))
        try write(plain, to: output)
    }
}
