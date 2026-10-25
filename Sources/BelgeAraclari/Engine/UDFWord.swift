import Foundation

/// UDF → Word (.docx). The writer lives in the shared Rust core (`core/`), which the build script
/// bundles next to the app executable as `belge-core`; Windows links the same code directly.
enum UDFWord {
    static func tool() throws -> URL {
        guard let url = Bundle.main.url(forAuxiliaryExecutable: "belge-core") else {
            throw AppError.message("Word dönüştürücüsü uygulama paketinde bulunamadı. Uygulamayı yeniden yükleyin.")
        }
        return url
    }

    /// Writes `urls` into `output` (several inputs become one document, one section each).
    static func write(_ urls: [URL], to output: URL) throws {
        let process = Process()
        process.executableURL = try tool()
        process.arguments = ["udf-to-docx", output.path] + urls.map(\.path)
        let errors = Pipe()
        process.standardError = errors
        process.standardOutput = FileHandle.nullDevice
        try process.run()
        let message = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let text = String(decoding: message, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            throw AppError.message(text.isEmpty ? "\"\(output.lastPathComponent)\" oluşturulamadı." : text)
        }
    }

    static func convert(_ urls: [URL], merge: Bool, mergedName: String, progress: ProgressReporter) throws -> [ResultItem] {
        let detail = { (url: URL, count: Int) in
            (count > 1 ? "\(count) UDF · " : "") + "Word belgesi · \(formatBytes(fileSize(url)))"
        }
        if merge, let first = urls.first {
            let name = mergedName.trimmingCharacters(in: .whitespaces).isEmpty ? Output.baseName(first) : mergedName
            let out = Output.uniqueFile(in: Output.directory(for: first), base: name, ext: "docx")
            progress(0.1, "Word belgesi oluşturuluyor…")
            try write(urls, to: out)
            return [ResultItem(title: out.lastPathComponent, detail: detail(out, urls.count), url: out)]
        }
        return urls.enumerated().map { i, url in
            progress(Double(i) / Double(urls.count), "Dönüştürülüyor: \(url.lastPathComponent)")
            let out = Output.uniqueFile(in: Output.directory(for: url), base: Output.baseName(url), ext: "docx")
            do {
                try write([url], to: out)
                return ResultItem(title: out.lastPathComponent, detail: detail(out, 1), url: out)
            } catch {
                return ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false)
            }
        }
    }
}
