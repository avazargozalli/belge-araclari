import Foundation
import ImageIO

enum PageSize: String, CaseIterable, Identifiable {
    case fit, a4, a5, letter, legal
    var id: String { rawValue }

    var title: String {
        switch self {
        case .fit: return "Resme göre"
        case .a4: return "A4"
        case .a5: return "A5"
        case .letter: return "Letter"
        case .legal: return "Legal"
        }
    }

    /// Portrait size in points; nil means "follow the image".
    var size: CGSize? {
        switch self {
        case .fit: return nil
        case .a4: return CGSize(width: 595.28, height: 841.89)
        case .a5: return CGSize(width: 419.53, height: 595.28)
        case .letter: return CGSize(width: 612, height: 792)
        case .legal: return CGSize(width: 612, height: 1008)
        }
    }
}

enum PageOrientation: String, CaseIterable, Identifiable {
    case auto, portrait, landscape
    var id: String { rawValue }
    var title: String {
        switch self {
        case .auto: return "Otomatik"
        case .portrait: return "Dikey"
        case .landscape: return "Yatay"
        }
    }
}

enum PageMargin: String, CaseIterable, Identifiable {
    case none, small, normal
    var id: String { rawValue }
    var title: String {
        switch self {
        case .none: return "Yok"
        case .small: return "Dar"
        case .normal: return "Normal"
        }
    }
    var points: CGFloat {
        switch self {
        case .none: return 0
        case .small: return 18
        case .normal: return 36
        }
    }
}

enum ImageQuality: String, CaseIterable, Identifiable {
    case original, high, balanced, small
    var id: String { rawValue }
    var title: String {
        switch self {
        case .original: return "Orijinal"
        case .high: return "Yüksek"
        case .balanced: return "Dengeli"
        case .small: return "Küçük dosya"
        }
    }
    var jpegQuality: Double {
        switch self {
        case .original: return 0.95
        case .high: return 0.85
        case .balanced: return 0.75
        case .small: return 0.6
        }
    }
    /// Longest side in pixels; nil keeps the original resolution.
    var maxPixel: Int? {
        switch self {
        case .original: return nil
        case .high: return 3200
        case .balanced: return 2200
        case .small: return 1600
        }
    }
}

struct ImagePDFOptions {
    var pageSize: PageSize = .a4
    var orientation: PageOrientation = .auto
    var margin: PageMargin = .none
    var quality: ImageQuality = .balanced
}

enum ImageToPDF {
    /// Builds one PDF from the images in order. Multi-page TIFF files contribute every frame.
    static func convert(_ urls: [URL], options: ImagePDFOptions, to output: URL, progress: ProgressReporter) throws -> Int {
        let writer = try PDFImageWriter(url: output, title: Output.baseName(output))
        var failed: [String] = []

        for (i, url) in urls.enumerated() {
            progress(Double(i) / Double(urls.count), "İşleniyor: \(url.lastPathComponent)")
            guard let source = CGImageSourceCreateWithURL(url as CFURL, nil), CGImageSourceGetCount(source) > 0 else {
                failed.append(url.lastPathComponent)
                continue
            }
            let isTIFF = ["tif", "tiff"].contains(url.pathExtension.lowercased())
            let frames = isTIFF ? CGImageSourceGetCount(source) : 1
            for frame in 0..<frames {
                autoreleasepool {
                    guard let loaded = ImageCodec.loadOriented(source, index: frame, maxPixel: options.quality.maxPixel),
                          let flat = ImageCodec.flattened(loaded),
                          let jpeg = ImageCodec.jpegData(flat, quality: options.quality.jpegQuality),
                          let image = ImageCodec.jpegBackedImage(jpeg) else {
                        failed.append(url.lastPathComponent)
                        return
                    }
                    let (pageSize, rect) = layout(imageSize: CGSize(width: image.width, height: image.height), options: options)
                    writer.addPage(size: pageSize, image: image, in: rect)
                }
            }
        }
        writer.close()

        guard writer.pageCount > 0 else {
            try? FileManager.default.removeItem(at: output)
            throw AppError.message("Resimler okunamadı. Desteklenen biçimler: JPG, PNG, WEBP, HEIC, TIFF, GIF, BMP.")
        }
        if !failed.isEmpty {
            NSLog("Belge Araçları: okunamayan resimler: \(failed.joined(separator: ", "))")
        }
        return writer.pageCount
    }

    static func layout(imageSize: CGSize, options: ImagePDFOptions) -> (CGSize, CGRect) {
        let margin = options.margin.points
        let isLandscapeImage = imageSize.width > imageSize.height

        var page: CGSize
        if let base = options.pageSize.size {
            let landscape: Bool
            switch options.orientation {
            case .auto: landscape = isLandscapeImage
            case .portrait: landscape = false
            case .landscape: landscape = true
            }
            page = landscape ? CGSize(width: base.height, height: base.width) : base
        } else {
            // Follow the image's aspect ratio with the long side at A4 height.
            let longSide: CGFloat = 841.89
            let scale = longSide / max(imageSize.width, imageSize.height)
            page = CGSize(width: imageSize.width * scale + margin * 2, height: imageSize.height * scale + margin * 2)
        }

        let content = CGRect(origin: .zero, size: page).insetBy(dx: margin, dy: margin)
        let scale = min(content.width / imageSize.width, content.height / imageSize.height)
        let drawn = CGSize(width: imageSize.width * scale, height: imageSize.height * scale)
        let rect = CGRect(x: content.midX - drawn.width / 2, y: content.midY - drawn.height / 2,
                          width: drawn.width, height: drawn.height)
        return (page, rect)
    }
}
