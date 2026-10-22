import Foundation
import AppKit
import PDFKit
import UniformTypeIdentifiers
import ImageIO

// MARK: - Errors

enum AppError: LocalizedError {
    case cannotOpen(String)
    case locked(String)
    case cannotWrite(String)
    case noPages
    case invalidRange(String)
    case wrongPassword(String)
    case message(String)

    var errorDescription: String? {
        switch self {
        case .cannotOpen(let name): return "\"\(name)\" açılamadı. Dosya bozuk veya desteklenmeyen bir biçimde olabilir."
        case .locked(let name): return "\"\(name)\" şifreli. Önce \"PDF Şifrele\" aracından şifresini kaldırın."
        case .cannotWrite(let name): return "\"\(name)\" kaydedilemedi. Klasöre yazma izni olduğundan emin olun."
        case .noPages: return "İşlenecek sayfa bulunamadı."
        case .invalidRange(let text): return "Sayfa aralığı anlaşılamadı: \(text). Örnek: 1-3, 5, 8-10"
        case .wrongPassword(let name): return "\"\(name)\" için girilen şifre yanlış."
        case .message(let text): return text
        }
    }
}

// MARK: - File types

enum FileTypes {
    static let pdf: [UTType] = [.pdf]
    static let images: [UTType] = [.jpeg, .png, .webP, .heic, .heif, .tiff, .gif, .bmp]
    static let imageExtensions: Set<String> = ["jpg", "jpeg", "png", "webp", "heic", "heif", "tif", "tiff", "gif", "bmp"]

    static func isPDF(_ url: URL) -> Bool { url.pathExtension.lowercased() == "pdf" }
    static func isImage(_ url: URL) -> Bool { imageExtensions.contains(url.pathExtension.lowercased()) }

    /// Replaces dropped folders with the files they contain (one level, name-sorted).
    static func expand(_ urls: [URL]) -> [URL] {
        var result: [URL] = []
        for url in urls {
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue {
                let items = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])) ?? []
                result += items.sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
            } else {
                result.append(url)
            }
        }
        return result
    }
}

// MARK: - Output location

enum SettingsKey {
    static let outputMode = "outputMode"          // 0: next to original, 1: chosen folder
    static let outputFolder = "outputFolder"
    static let useGhostscript = "useGhostscript"
}

enum Output {
    /// Folder where results for `input` are written. Never overwrites anything — see `uniqueFile`.
    static func directory(for input: URL) -> URL {
        let defaults = UserDefaults.standard
        if defaults.integer(forKey: SettingsKey.outputMode) == 1,
           let path = defaults.string(forKey: SettingsKey.outputFolder), !path.isEmpty {
            let folder = URL(fileURLWithPath: path, isDirectory: true)
            if FileManager.default.isWritableFile(atPath: folder.path) { return folder }
        }
        let parent = input.deletingLastPathComponent()
        if FileManager.default.isWritableFile(atPath: parent.path) { return parent }
        return FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
    }

    static func uniqueFile(in dir: URL, base: String, ext: String) -> URL {
        let clean = sanitize(base)
        var url = dir.appendingPathComponent(clean).appendingPathExtension(ext)
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent("\(clean) (\(n))").appendingPathExtension(ext)
            n += 1
        }
        return url
    }

    static func uniqueFolder(in dir: URL, base: String) throws -> URL {
        let clean = sanitize(base)
        var url = dir.appendingPathComponent(clean, isDirectory: true)
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent("\(clean) (\(n))", isDirectory: true)
            n += 1
        }
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    static func baseName(_ url: URL) -> String { url.deletingPathExtension().lastPathComponent }

    private static func sanitize(_ name: String) -> String {
        let cleaned = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? "Belge" : cleaned
    }

    static func tempDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("BelgeAraclari-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

func fileSize(_ url: URL) -> Int64 {
    (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0
}

func formatBytes(_ bytes: Int64) -> String {
    let formatter = ByteCountFormatter()
    formatter.countStyle = .file
    return formatter.string(fromByteCount: bytes)
}

// MARK: - Progress

struct ProgressReporter: Sendable {
    let update: @Sendable (Double, String) -> Void
    func callAsFunction(_ fraction: Double, _ status: String) { update(fraction, status) }
}

// MARK: - Image encoding

enum ImageCodec {
    static func jpegData(_ image: CGImage, quality: Double) -> Data? {
        encode(image, type: UTType.jpeg, options: [kCGImageDestinationLossyCompressionQuality: quality])
    }

    static func pngData(_ image: CGImage) -> Data? {
        encode(image, type: UTType.png, options: [:])
    }

    private static func encode(_ image: CGImage, type: UTType, options: [CFString: Any]) -> Data? {
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, type.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dest, image, options as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { return nil }
        return data as Data
    }

    /// A CGImage backed by JPEG bytes. Quartz embeds such images in a PDF as-is (DCTDecode)
    /// instead of re-encoding them losslessly, which keeps the output small.
    static func jpegBackedImage(_ data: Data) -> CGImage? {
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(jpegDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// Draws the image on an opaque white RGB canvas so transparent areas don't turn black in JPEG.
    static func flattened(_ image: CGImage) -> CGImage? {
        let width = image.width, height = image.height
        guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
        ctx.interpolationQuality = .high
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return ctx.makeImage()
    }

    /// Loads one frame with EXIF orientation applied, downscaled so its long side is at most `maxPixel`.
    static func loadOriented(_ source: CGImageSource, index: Int, maxPixel: Int?) -> CGImage? {
        let props = CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any] ?? [:]
        let width = props[kCGImagePropertyPixelWidth] as? Int ?? 0
        let height = props[kCGImagePropertyPixelHeight] as? Int ?? 0
        let longSide = max(width, height)
        guard longSide > 0 else { return CGImageSourceCreateImageAtIndex(source, index, nil) }
        let target = min(longSide, maxPixel ?? longSide)
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: target,
            kCGImageSourceShouldCacheImmediately: true
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, index, options as CFDictionary)
    }
}

// MARK: - PDF writer for image pages

/// Writes a PDF where each page is a single image. Used by image→PDF and the scanned-document compressor.
final class PDFImageWriter {
    private let context: CGContext
    private(set) var pageCount = 0

    init(url: URL, title: String) throws {
        var box = CGRect(x: 0, y: 0, width: 595, height: 842)
        let info: [CFString: Any] = [kCGPDFContextCreator: PDFEngine.creator, kCGPDFContextTitle: title]
        guard let ctx = CGContext(url as CFURL, mediaBox: &box, info as CFDictionary) else {
            throw AppError.cannotWrite(url.lastPathComponent)
        }
        context = ctx
    }

    func addPage(size: CGSize, image: CGImage, in rect: CGRect) {
        var box = CGRect(origin: .zero, size: size)
        let boxData = Data(bytes: &box, count: MemoryLayout<CGRect>.size)
        context.beginPDFPage([kCGPDFContextMediaBox: boxData] as CFDictionary)
        context.interpolationQuality = .high
        context.draw(image, in: rect)
        context.endPDFPage()
        pageCount += 1
    }

    func close() { context.closePDF() }
}
