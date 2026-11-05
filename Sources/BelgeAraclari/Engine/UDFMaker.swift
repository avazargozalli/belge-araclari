import Foundation
import PDFKit

/// PDF and Word documents → UYAP UDF, through the shared core ([BelgeCore]).
enum UDFMaker {
    enum PDFMode: String, CaseIterable { case text, pages }

    static func isSource(_ url: URL) -> Bool {
        FileTypes.isPDF(url) || ["doc", "docx", "docm", "dot", "dotx", "dotm", "rtf", "odt", "ott"].contains(url.pathExtension.lowercased())
    }

    /// Every page as a picture: the exact look, not editable.
    private static func pages(_ url: URL, to output: URL) throws {
        guard let doc = PDFDocument(url: url) else { throw AppError.cannotOpen(url.lastPathComponent) }
        if doc.isLocked { throw AppError.locked(url.lastPathComponent) }
        let temp = try Output.tempDirectory()
        defer { try? FileManager.default.removeItem(at: temp) }
        var args = ["pages-to-udf", output.path]
        for i in 0..<doc.pageCount {
            guard let page = doc.page(at: i), let image = PDFEngine.renderPage(page, dpi: 120), let jpeg = ImageCodec.jpegData(image, quality: 0.75) else { continue }
            let file = temp.appendingPathComponent("\(i).jpg")
            try jpeg.write(to: file)
            let box = page.bounds(for: .cropBox)
            let rotated = page.rotation % 180 != 0
            args += [file.path, String(format: "%.2f", rotated ? box.height : box.width), String(format: "%.2f", rotated ? box.width : box.height)]
        }
        guard args.count > 2 else { throw AppError.noPages }
        let result = try BelgeCore.run(args)
        guard result.status == 0 else { throw AppError.message(result.stderr.isEmpty ? "\"\(output.lastPathComponent)\" oluşturulamadı." : result.stderr) }
    }

    static func convert(_ urls: [URL], pdfMode: PDFMode, useInstalled: Bool, progress: ProgressReporter) -> [ResultItem] {
        urls.enumerated().map { i, url in
            progress(Double(i) / Double(max(urls.count, 1)), "Dönüştürülüyor: \(url.lastPathComponent)")
            let out = Output.uniqueFile(in: Output.directory(for: url), base: Output.baseName(url), ext: "udf")
            do {
                var how: String
                if FileTypes.isPDF(url) && pdfMode == .pages {
                    try pages(url, to: out)
                    how = "sayfalar resim olarak"
                } else {
                    let result = try BelgeCore.run(["to-udf", url.path, out.path] + (useInstalled ? [] : ["--no-office"]))
                    guard result.status == 0 else {
                        throw AppError.message(result.stderr.isEmpty ? "\"\(url.lastPathComponent)\" çevrilemedi." : result.stderr)
                    }
                    how = result.stdout
                }
                return ResultItem(title: out.lastPathComponent, detail: "UDF · \(formatBytes(fileSize(out))) · \(how)", url: out)
            } catch {
                try? FileManager.default.removeItem(at: out)
                return ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false)
            }
        }
    }
}
