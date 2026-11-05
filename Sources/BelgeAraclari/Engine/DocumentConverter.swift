import Foundation
import CoreGraphics

/// Turns any supported file into a PDF: PDFs are used as they are, images go through ImageToPDF,
/// UDF through the TextKit renderer, office files through an installed office suite or the
/// built-in Word reader. Results are saved one by one or merged in list order.
enum DocumentConverter {
    /// Converts one office file. Returns how it was done, for the result line.
    static func office(_ url: URL, to output: URL, useInstalled: Bool) throws -> String {
        var suiteError: String?
        if useInstalled {
            let result = try BelgeCore.run(["office-to-pdf", url.path, output.path])
            switch result.status {
            case 0: return result.stdout.isEmpty ? "Office" : result.stdout
            case 3: break  // no installed suite can open it
            default: suiteError = result.stderr
            }
        }
        guard FileTypes.isBuiltinWord(url) else {
            throw AppError.message(suiteError ?? (useInstalled
                ? "\"\(url.lastPathComponent)\" için Microsoft Office veya LibreOffice gerekir. Yerleşik dönüştürücü yalnızca Word (.docx) dosyalarını çevirebilir."
                : "\"\(url.lastPathComponent)\" için kurulu Office kullanımı Ayarlar'da kapalı. Yerleşik dönüştürücü yalnızca Word (.docx) dosyalarını çevirebilir."))
        }
        let temp = try Output.tempDirectory()
        defer { try? FileManager.default.removeItem(at: temp) }
        let udf = temp.appendingPathComponent("belge.udf")
        let result = try BelgeCore.run(["docx-to-udf", url.path, udf.path])
        guard result.status == 0 else { throw AppError.message(result.stderr.isEmpty ? "\"\(url.lastPathComponent)\" okunamadı." : result.stderr) }
        try UDFRenderer.render(udf, to: output)
        return suiteError == nil ? "yerleşik dönüştürücü" : "yerleşik dönüştürücü (Office başarısız oldu)"
    }

    /// Writes one input as a PDF at `output`. Returns a short note on how, or nil for PDFs.
    @discardableResult
    static func pdf(_ url: URL, to output: URL, imagePage: PageSize, useInstalled: Bool) throws -> String? {
        if FileTypes.isPDF(url) {
            try FileManager.default.copyItem(at: url, to: output)
            return nil
        }
        if FileTypes.isImage(url) {
            var options = ImagePDFOptions()
            options.pageSize = imagePage
            options.margin = imagePage == .fit ? .none : .small
            _ = try ImageToPDF.convert([url], options: options, to: output, progress: ProgressReporter { _, _ in })
            return "resim"
        }
        if FileTypes.isUDF(url) {
            try UDFRenderer.render(url, to: output)
            return "UDF"
        }
        if FileTypes.isOffice(url) {
            return try office(url, to: output, useInstalled: useInstalled)
        }
        throw AppError.message("\"\(url.lastPathComponent)\" desteklenmeyen bir dosya türü.")
    }

    static func convert(_ urls: [URL], merge: Bool, mergedName: String, imagePage: PageSize, useInstalled: Bool,
                        numbering: PageNumbering = PageNumbering(), progress: ProgressReporter) throws -> [ResultItem] {
        let temp = try Output.tempDirectory()
        defer { try? FileManager.default.removeItem(at: temp) }
        var results: [ResultItem] = []
        var parts: [URL] = []
        for (i, url) in urls.enumerated() {
            progress(Double(i) / Double(max(urls.count, 1)) * (merge ? 0.9 : 1), "Dönüştürülüyor: \(url.lastPathComponent)")
            do {
                if merge {
                    let out = temp.appendingPathComponent("\(i).pdf")
                    try pdf(url, to: out, imagePage: imagePage, useInstalled: useInstalled)
                    parts.append(out)
                } else if FileTypes.isPDF(url) && !numbering.enabled {
                    results.append(ResultItem(title: url.lastPathComponent, detail: "Zaten PDF; olduğu gibi bırakıldı.", url: url))
                } else if FileTypes.isPDF(url) {
                    // The original is never changed: the numbered PDF is a copy.
                    let out = Output.uniqueFile(in: Output.directory(for: url), base: Output.baseName(url) + " - numaralı", ext: "pdf")
                    try FileManager.default.copyItem(at: url, to: out)
                    let pages = CGPDFDocument(out as CFURL)?.numberOfPages ?? 0
                    results.append(ResultItem(title: out.lastPathComponent, detail: "\(pages) sayfa · \(formatBytes(fileSize(out)))", url: out))
                } else {
                    let out = Output.uniqueFile(in: Output.directory(for: url), base: Output.baseName(url), ext: "pdf")
                    let how = try pdf(url, to: out, imagePage: imagePage, useInstalled: useInstalled)
                    let pages = CGPDFDocument(out as CFURL)?.numberOfPages ?? 0
                    var detail = "\(pages) sayfa · \(formatBytes(fileSize(out)))"
                    if let how, FileTypes.isOffice(url) { detail += " · \(how)" }
                    results.append(ResultItem(title: out.lastPathComponent, detail: detail, url: out))
                }
            } catch {
                results.append(ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false))
            }
        }
        if merge, let first = urls.first {
            guard !parts.isEmpty else { return numbering.apply(results, skip: Set(urls)) }
            let name = mergedName.trimmingCharacters(in: .whitespaces).isEmpty ? Output.baseName(first) : mergedName
            let out = Output.uniqueFile(in: Output.directory(for: first), base: name, ext: "pdf")
            progress(0.95, "Birleştiriliyor…")
            let pages = try PDFEngine.merge(parts, to: out, progress: ProgressReporter { _, _ in })
            results.insert(ResultItem(title: out.lastPathComponent,
                                      detail: "\(parts.count) dosya · \(pages) sayfa · \(formatBytes(fileSize(out)))", url: out), at: 0)
        }
        return numbering.apply(results, skip: Set(urls))
    }
}
