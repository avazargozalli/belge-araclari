import Foundation
import PDFKit
import Quartz

enum CompressionLevel: String, CaseIterable, Identifiable {
    case balanced, smaller, smallest
    var id: String { rawValue }

    var title: String {
        switch self {
        case .balanced: return "Makul"
        case .smaller: return "Daha Küçük"
        case .smallest: return "En Küçük"
        }
    }

    var subtitle: String {
        switch self {
        case .balanced: return "Kalite ve boyut arasında önerilen dengeli seçim."
        case .smaller: return "Görsel kaliteyi biraz azaltarak daha fazla küçültür."
        case .smallest: return "En agresif sıkıştırma. Görsel kalite belirgin düşebilir."
        }
    }

    var jpegQuality: Double {
        switch self {
        case .balanced: return 0.72
        case .smaller: return 0.55
        case .smallest: return 0.4
        }
    }

    var dpi: Int {
        switch self {
        case .balanced: return 150
        case .smaller: return 110
        case .smallest: return 72
        }
    }

    /// Resolution used when whole pages are turned into images (scanned-document mode).
    var scanDPI: CGFloat {
        switch self {
        case .balanced: return 150
        case .smaller: return 120
        case .smallest: return 96
        }
    }

    var ghostscriptPreset: String { self == .balanced ? "/ebook" : "/screen" }
}

struct CompressionOutcome {
    let input: URL
    let output: URL?
    let originalSize: Int64
    let newSize: Int64
    let method: String
}

/// Tries several local strategies and keeps the smallest valid result.
/// The original file is never modified; if nothing is smaller, no file is written.
enum Compressor {
    static func ghostscriptPath() -> String? {
        ["/opt/homebrew/bin/gs", "/usr/local/bin/gs", "/opt/local/bin/gs"]
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    static func compress(_ url: URL, level: CompressionLevel, scanMode: Bool, useGhostscript: Bool,
                         progress: @escaping (String) -> Void) throws -> CompressionOutcome {
        let doc = try PDFEngine.open(url)
        let pageCount = doc.pageCount
        guard pageCount > 0 else { throw AppError.noPages }
        let originalSize = fileSize(url)

        let temp = try Output.tempDirectory()
        defer { try? FileManager.default.removeItem(at: temp) }

        var candidates: [(url: URL, method: String)] = []

        if scanMode {
            progress("Sayfalar görüntüye çevriliyor…")
            let out = temp.appendingPathComponent("scan.pdf")
            if (try? rasterize(doc, level: level, to: out, title: Output.baseName(url))) != nil {
                candidates.append((out, "Taranmış belge modu"))
            }
        } else {
            progress("Görseller sıkıştırılıyor…")
            let quartzOut = temp.appendingPathComponent("quartz.pdf")
            if let reloaded = PDFDocument(url: url), quartzFilterCompress(reloaded, level: level, to: quartzOut) {
                candidates.append((quartzOut, "Yerleşik motor"))
            }

            progress("Alternatif yöntem deneniyor…")
            let kitOut = temp.appendingPathComponent("pdfkit.pdf")
            if let reloaded = PDFDocument(url: url),
               reloaded.write(to: kitOut, withOptions: [.saveImagesAsJPEGOption: true, .optimizeImagesForScreenOption: level != .balanced]) {
                candidates.append((kitOut, "PDFKit optimizasyonu"))
            }

            if useGhostscript, let gs = ghostscriptPath() {
                progress("Ghostscript ile sıkıştırılıyor…")
                let gsOut = temp.appendingPathComponent("gs.pdf")
                if ghostscriptCompress(gs: gs, input: url, output: gsOut, level: level) {
                    candidates.append((gsOut, "Ghostscript"))
                }
            }
        }

        progress("Sonuç doğrulanıyor…")
        let valid = candidates.compactMap { candidate -> (URL, String, Int64)? in
            let size = fileSize(candidate.url)
            guard size > 0, let check = PDFDocument(url: candidate.url), check.pageCount == pageCount else { return nil }
            return (candidate.url, candidate.method, size)
        }

        guard let best = valid.min(by: { $0.2 < $1.2 }), best.2 < originalSize else {
            return CompressionOutcome(input: url, output: nil, originalSize: originalSize, newSize: originalSize, method: "")
        }

        let suffix = scanMode ? "küçültülmüş-taranmış" : "küçültülmüş"
        let destination = Output.uniqueFile(in: Output.directory(for: url), base: "\(Output.baseName(url)) - \(suffix)", ext: "pdf")
        do {
            try FileManager.default.copyItem(at: best.0, to: destination)
        } catch {
            throw AppError.cannotWrite(destination.lastPathComponent)
        }
        return CompressionOutcome(input: url, output: destination, originalSize: originalSize, newSize: best.2, method: best.1)
    }

    /// Re-encodes images through a Quartz filter (same mechanism as Preview's "Reduce File Size"),
    /// with tunable JPEG quality and resolution. Text and vector content stay intact.
    private static func quartzFilterCompress(_ doc: PDFDocument, level: CompressionLevel, to output: URL) -> Bool {
        let maxPixels: Int
        switch level {
        case .balanced: maxPixels = 2400
        case .smaller: maxPixels = 1700
        case .smallest: maxPixels = 1200
        }
        let properties: [String: Any] = [
            "Domains": ["Applications": true, "Printing": true],
            "FilterData": [
                "ColorSettings": [
                    "ImageSettings": [
                        "Compression Quality": level.jpegQuality,
                        "ImageCompression": "ImageJPEGCompress",
                        "ImageScaleSettings": [
                            "ImageResolution": level.dpi,
                            "ImageScaleInterpolate": true,
                            "ImageSizeMax": maxPixels,
                            "ImageSizeMin": 0
                        ]
                    ]
                ]
            ],
            "FilterType": 1,
            "Name": "Belge Araclari"
        ]
        guard let filter = QuartzFilter(properties: properties) else { return false }
        return doc.write(to: output, withOptions: [PDFDocumentWriteOption(rawValue: "QuartzFilter"): filter])
    }

    private static func ghostscriptCompress(gs: String, input: URL, output: URL, level: CompressionLevel) -> Bool {
        var args = [
            "-sDEVICE=pdfwrite",
            "-dCompatibilityLevel=1.5",
            "-dPDFSETTINGS=\(level.ghostscriptPreset)",
            "-dDetectDuplicateImages=true",
            "-dCompressFonts=true",
            "-dSubsetFonts=true",
            "-dSAFER",
            "-dNOPAUSE",
            "-dQUIET",
            "-dBATCH"
        ]
        if level != .balanced {
            let color = level.dpi, mono = level == .smallest ? 150 : 200
            args += ["-dColorImageResolution=\(color)", "-dGrayImageResolution=\(color)", "-dMonoImageResolution=\(mono)"]
        }
        args += ["-sOutputFile=\(output.path)", input.path]

        let process = Process()
        process.executableURL = URL(fileURLWithPath: gs)
        process.arguments = args
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return false
        }
        return process.terminationStatus == 0 && fileSize(output) > 0
    }

    /// Scanned-document mode: every page becomes one JPEG image. Very effective for scans and photos,
    /// but text is no longer selectable.
    private static func rasterize(_ doc: PDFDocument, level: CompressionLevel, to output: URL, title: String) throws {
        let writer = try PDFImageWriter(url: output, title: title)
        for index in 0..<doc.pageCount {
            try autoreleasepool {
                guard let page = doc.page(at: index),
                      let bitmap = PDFEngine.renderPage(page, dpi: level.scanDPI),
                      let jpeg = ImageCodec.jpegData(bitmap, quality: level.jpegQuality),
                      let image = ImageCodec.jpegBackedImage(jpeg) else { throw AppError.noPages }
                let size = PDFEngine.displaySize(page)
                writer.addPage(size: size, image: image, in: CGRect(origin: .zero, size: size))
            }
        }
        writer.close()
    }
}
