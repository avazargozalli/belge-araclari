import Foundation
import AppKit
import Compression

// UYAP Doküman Formatı (.udf): a ZIP holding content.xml. The XML keeps the whole text in one CDATA
// block; <elements> describes paragraphs and runs by UTF-16 offset, each with Java Swing style
// attributes (UYAP's editor is a Swing StyledDocument). This file parses that and lays it out with
// TextKit so the PDF keeps the original fonts, alignment, indents, tabs, letterhead and page breaks.

// MARK: - ZIP

enum MiniZip {
    /// Reads every stored or deflated entry. Enough for UDF files; no external tools involved.
    static func entries(_ data: Data, maxEntrySize: Int = 256 << 20) throws -> [String: Data] {
        let bytes = [UInt8](data)
        func u16(_ i: Int) -> Int { i + 1 < bytes.count ? Int(bytes[i]) | Int(bytes[i + 1]) << 8 : 0 }
        func u32(_ i: Int) -> Int { i + 3 < bytes.count ? u16(i) | u16(i + 2) << 16 : 0 }

        var eocd = -1
        var i = bytes.count - 22
        while i >= max(0, bytes.count - 65_557) {
            if u32(i) == 0x0605_4B50 { eocd = i; break }
            i -= 1
        }
        guard eocd >= 0 else { throw AppError.message("UDF dosyası okunamadı (ZIP yapısı bulunamadı).") }

        var result: [String: Data] = [:]
        var p = u32(eocd + 16)
        for _ in 0..<u16(eocd + 10) {
            guard u32(p) == 0x0201_4B50 else { break }
            let method = u16(p + 10)
            let compressed = u32(p + 20), size = u32(p + 24)
            let nameLength = u16(p + 28), extraLength = u16(p + 30), commentLength = u16(p + 32)
            let local = u32(p + 42)
            let name = String(decoding: bytes[(p + 46)..<min(bytes.count, p + 46 + nameLength)], as: UTF8.self)
            p += 46 + nameLength + extraLength + commentLength

            guard u32(local) == 0x0403_4B50, size <= maxEntrySize else { continue }
            let start = local + 30 + u16(local + 26) + u16(local + 28)
            guard start + compressed <= bytes.count else { continue }
            let slice = Array(bytes[start..<(start + compressed)])
            if method == 0 {
                result[name] = Data(slice)
            } else if method == 8 {
                var out = [UInt8](repeating: 0, count: max(size, 1))
                let written = compression_decode_buffer(&out, out.count, slice, slice.count, nil, COMPRESSION_ZLIB)
                if written == size { result[name] = Data(out.prefix(size)) }
            }
        }
        return result
    }
}

// MARK: - Model

struct UDFRun {
    enum Kind { case text, image(Data, CGSize) }
    let kind: Kind
    let range: NSRange
    let attrs: [String: String]
}

struct UDFParagraph {
    let attrs: [String: String]
    let runs: [UDFRun]
}

struct UDFTable {
    let attrs: [String: String]
    let rows: [[[UDFParagraph]]]
}

enum UDFBlock {
    case paragraph(UDFParagraph)
    case table(UDFTable)
}

struct UDFPageNumber {
    let leading: String
    let separator: String
    let showTotal: Bool
    let alignment: NSTextAlignment
    let font: NSFont
    let color: NSColor
    let start: Int
}

struct UDFDocument {
    var text: NSString = ""
    var pageSize = CGSize(width: 595.28, height: 841.89)
    var margins = NSEdgeInsets(top: 42.5, left: 42.5, bottom: 42.5, right: 42.5)
    var headerOffset: CGFloat = 20
    var footerOffset: CGFloat = 20
    var background: (image: CGImage, insets: NSEdgeInsets)?
    var header: [UDFBlock] = []
    var body: [UDFBlock] = []
    var footer: [UDFBlock] = []
    var headerPageNumber: UDFPageNumber?
    var footerPageNumber: UDFPageNumber?
    var styles: [String: [String: String]] = [:]
    var baseResolver = "hvl-default"
}

// MARK: - Parsing

enum UDFParser {
    static func parse(_ url: URL) throws -> UDFDocument {
        let name = url.lastPathComponent
        guard let data = try? Data(contentsOf: url) else { throw AppError.cannotOpen(name) }
        guard let xmlData = try MiniZip.entries(data)["content.xml"] else {
            throw AppError.message("\"\(name)\" geçerli bir UDF değil (content.xml yok).")
        }
        let xml: XMLDocument
        do {
            xml = try XMLDocument(data: xmlData, options: [.nodePreserveCDATA])
        } catch {
            throw AppError.message("\"\(name)\" okunamadı: XML bozuk.")
        }
        guard let root = xml.rootElement() else { throw AppError.cannotOpen(name) }

        var doc = UDFDocument()
        doc.text = (root.elements(forName: "content").first?.stringValue ?? "") as NSString

        if let styles = root.elements(forName: "styles").first {
            for style in styles.elements(forName: "style") {
                let attrs = attributes(of: style)
                if let styleName = attrs["name"] { doc.styles[styleName] = attrs }
            }
        }

        if let properties = root.elements(forName: "properties").first {
            if let format = properties.elements(forName: "pageFormat").first {
                let a = attributes(of: format)
                var size = CGSize(width: 595.28, height: 841.89)
                if a["paperOrientation"] == "0" { size = CGSize(width: size.height, height: size.width) }
                doc.pageSize = size
                doc.margins = NSEdgeInsets(top: number(a["topMargin"], 42.5), left: number(a["leftMargin"], 42.5),
                                           bottom: number(a["bottomMargin"], 42.5), right: number(a["rightMargin"], 42.5))
                doc.headerOffset = number(a["headerFOffset"], 20)
                doc.footerOffset = number(a["footerFOffset"], 20)
            }
            if let bg = properties.elements(forName: "bgImage").first {
                let a = attributes(of: bg)
                if let image = decodeImage(a["bgImageData"]) {
                    doc.background = (image, NSEdgeInsets(top: number(a["bgImageUpMargin"], 0), left: number(a["bgImageLeftMargin"], 0),
                                                          bottom: number(a["bgImageBottomMargin"], 0), right: number(a["bgImageRigtMargin"], 0)))
                }
            }
        }

        guard let elements = root.elements(forName: "elements").first else { return doc }
        if let resolver = elements.attribute(forName: "resolver")?.stringValue, !resolver.isEmpty { doc.baseResolver = resolver }
        let base = resolvedStyle(doc.styles, "default").merging(resolvedStyle(doc.styles, doc.baseResolver)) { $1 }

        for child in elements.children?.compactMap({ $0 as? XMLElement }) ?? [] {
            switch child.name {
            case "header":
                doc.header = blocks(in: child, base: base, styles: doc.styles)
                doc.headerPageNumber = pageNumber(attributes(of: child))
            case "footer":
                doc.footer = blocks(in: child, base: base, styles: doc.styles)
                doc.footerPageNumber = pageNumber(attributes(of: child))
            case "paragraph":
                doc.body.append(.paragraph(paragraph(child, base: base, styles: doc.styles)))
            case "table":
                doc.body.append(.table(table(child, base: base, styles: doc.styles)))
            default:
                break
            }
        }
        return doc
    }

    private static func attributes(of element: XMLElement) -> [String: String] {
        var result: [String: String] = [:]
        for attribute in element.attributes ?? [] {
            if let name = attribute.name { result[name] = attribute.stringValue ?? "" }
        }
        return result
    }

    static func number(_ value: String?, _ fallback: CGFloat) -> CGFloat {
        guard let value, let d = Double(value), d.isFinite else { return fallback }
        return CGFloat(d)
    }

    static func resolvedStyle(_ styles: [String: [String: String]], _ name: String?, depth: Int = 0) -> [String: String] {
        guard let name, let style = styles[name], depth < 12 else { return [:] }
        var result = resolvedStyle(styles, style["resolver"], depth: depth + 1)
        result.merge(style) { $1 }
        return result
    }

    private static func blocks(in element: XMLElement, base: [String: String], styles: [String: [String: String]]) -> [UDFBlock] {
        (element.children ?? []).compactMap { $0 as? XMLElement }.compactMap { child in
            switch child.name {
            case "paragraph": return .paragraph(paragraph(child, base: base, styles: styles))
            case "table": return .table(table(child, base: base, styles: styles))
            default: return nil
            }
        }
    }

    private static func paragraph(_ element: XMLElement, base: [String: String], styles: [String: [String: String]]) -> UDFParagraph {
        let own = attributes(of: element)
        let attrs = base.merging(resolvedStyle(styles, own["resolver"])) { $1 }.merging(own) { $1 }
        var runs: [UDFRun] = []
        for child in (element.children ?? []).compactMap({ $0 as? XMLElement }) {
            let a = attributes(of: child)
            guard let start = Int(a["startOffset"] ?? ""), let length = Int(a["length"] ?? ""), start >= 0, length >= 0 else { continue }
            let runAttrs = attrs.merging(resolvedStyle(styles, a["resolver"])) { $1 }.merging(a) { $1 }
            if child.name == "image", let data = imageData(a["imageData"]) {
                let size = CGSize(width: number(a["width"], 0), height: number(a["height"], 0))
                runs.append(UDFRun(kind: .image(data, size), range: NSRange(location: start, length: length), attrs: runAttrs))
            } else {
                runs.append(UDFRun(kind: .text, range: NSRange(location: start, length: length), attrs: runAttrs))
            }
        }
        return UDFParagraph(attrs: attrs, runs: runs)
    }

    private static func table(_ element: XMLElement, base: [String: String], styles: [String: [String: String]]) -> UDFTable {
        let rows = element.elements(forName: "row").map { row in
            row.elements(forName: "cell").map { cell in
                cell.elements(forName: "paragraph").map { paragraph($0, base: base, styles: styles) }
            }
        }
        return UDFTable(attrs: attributes(of: element), rows: rows)
    }

    private static func pageNumber(_ a: [String: String]) -> UDFPageNumber? {
        guard let spec = a["pageNumber-spec"], let flags = Int(spec.split(separator: "_").last ?? ""), flags & 8 != 0 else { return nil }
        let size = number(a["pageNumber-fontSize"], 11)
        var font = NSFont(name: a["pageNumber-fontFace"] ?? "Arial", size: size) ?? .systemFont(ofSize: size)
        if a["pageNumber-fontBold"] == "true" { font = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask) }
        if a["pageNumber-fontItalic"] == "true" { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        let alignment: NSTextAlignment = flags & 16 != 0 ? .left : (flags & 32 != 0 ? .right : .center)
        return UDFPageNumber(leading: a["pageNumber-foreStr"] ?? "", separator: a["pageNumber-seperator"] ?? "/",
                             showTotal: flags & 2048 != 0, alignment: alignment, font: font,
                             color: color(a["pageNumber-color"]) ?? .black, start: Int(a["pageNumber-pageStartNumStr"] ?? "") ?? 1)
    }

    static func imageData(_ base64: String?) -> Data? {
        guard let base64, !base64.isEmpty else { return nil }
        return Data(base64Encoded: base64, options: .ignoreUnknownCharacters)
    }

    private static func decodeImage(_ base64: String?) -> CGImage? {
        guard let data = imageData(base64), let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        return CGImageSourceCreateImageAtIndex(source, 0, nil)
    }

    /// Java colours are signed ARGB integers.
    static func color(_ value: String?) -> NSColor? {
        guard let value, let int = Int64(value) else { return nil }
        let argb = UInt32(truncatingIfNeeded: int)
        return NSColor(srgbRed: CGFloat((argb >> 16) & 0xFF) / 255, green: CGFloat((argb >> 8) & 0xFF) / 255,
                       blue: CGFloat(argb & 0xFF) / 255, alpha: 1)
    }
}

// MARK: - Attributed text

private struct ListCounter {
    var counts: [String: Int] = [:]
    mutating func next(_ key: String) -> Int {
        counts[key, default: 0] += 1
        return counts[key]!
    }
}

enum UDFTypesetter {
    static let fallbackFamily = "Times New Roman"

    static func font(_ a: [String: String]) -> NSFont {
        let size = max(1, UDFParser.number(a["size"], 12))
        var family = a["family"] ?? fallbackFamily
        if family == "Dialog" || family == "SansSerif" { family = "Arial" }
        if family == "Serif" { family = "Times New Roman" }
        if family == "Monospaced" || family == "DialogInput" { family = "Courier New" }
        var font = NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: size)
            ?? NSFont(name: family, size: size)
            ?? NSFont(name: fallbackFamily, size: size)
            ?? .systemFont(ofSize: size)
        if a["bold"] == "true" { font = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask) }
        if a["italic"] == "true" { font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask) }
        return font
    }

    static func paragraphStyle(_ a: [String: String], width: CGFloat) -> NSMutableParagraphStyle {
        let style = NSMutableParagraphStyle()
        switch a["Alignment"] {
        case "1": style.alignment = .center
        case "2": style.alignment = .right
        case "3": style.alignment = .justified
        default: style.alignment = .left
        }
        let left = max(0, UDFParser.number(a["LeftIndent"], 0))
        let right = max(0, UDFParser.number(a["RightIndent"], 0))
        let first = UDFParser.number(a["FirstLineIndent"], 0)
        let hanging = max(0, UDFParser.number(a["Hanging"], 0))
        style.firstLineHeadIndent = max(0, left + first)
        style.headIndent = left + hanging
        style.tailIndent = -right
        // Swing LineSpacing is extra space as a fraction of the line height.
        let extra = UDFParser.number(a["LineSpacing"], 0)
        style.lineHeightMultiple = max(0.7, 1 + (extra < -0.3 ? 0 : extra))
        style.paragraphSpacingBefore = max(0, UDFParser.number(a["SpaceAbove"], 0))
        style.paragraphSpacing = max(0, UDFParser.number(a["SpaceBelow"], 0))

        // TabSet = "position:alignment:leader,…"; without one Swing uses a stop every 72 pt.
        let stops: [NSTextTab] = (a["TabSet"] ?? "").split(separator: ",").compactMap { item in
            let parts = item.split(separator: ":")
            guard let position = Double(parts.first ?? "") else { return nil }
            let alignment: NSTextAlignment = parts.count > 1 && parts[1] == "1" ? .right : (parts.count > 1 && parts[1] == "2" ? .center : .left)
            return NSTextTab(textAlignment: alignment, location: CGFloat(position) + left)
        }
        style.tabStops = stops
        style.defaultTabInterval = stops.isEmpty ? 72 : 5
        style.lineBreakMode = .byWordWrapping
        return style
    }

    private static func listLabel(_ a: [String: String], counter: inout ListCounter) -> String? {
        let key = (a["ListId"] ?? "0") + ":" + (a["ListLevel"] ?? "1")
        if a["Bulleted"] == "true" {
            return "•"
        }
        guard a["Numbered"] == "true" else { return nil }
        let n = counter.next(key)
        switch a["NumberType"] ?? a["SecListTypeLevel1"] {
        case "NUMBER_TYPE_NUMBER_TRE": return "\(n)-"
        case "NUMBER_TYPE_NUMBER_PARANTEZ": return "\(n))"
        default: return "\(n)."
        }
    }

    /// Characters in the Private Use Area render as empty boxes in standard fonts; UYAP shows nothing.
    private static func cleaned(_ text: String) -> String {
        String(String.UnicodeScalarView(text.unicodeScalars.filter { !(0xE000...0xF8FF).contains($0.value) && $0 != "\r" }))
    }

    static func attributed(_ blocks: [UDFBlock], doc: UDFDocument, width: CGFloat) -> NSAttributedString {
        let result = NSMutableAttributedString()
        var counter = ListCounter()
        for block in blocks {
            switch block {
            case .paragraph(let p):
                result.append(paragraph(p, doc: doc, width: width, counter: &counter, block: nil))
            case .table(let t):
                result.append(table(t, doc: doc, width: width, counter: &counter))
            }
        }
        return result
    }

    private static func runAttributes(_ a: [String: String], paragraphStyle: NSParagraphStyle) -> [NSAttributedString.Key: Any] {
        var attrs: [NSAttributedString.Key: Any] = [
            .font: font(a),
            .paragraphStyle: paragraphStyle,
            .foregroundColor: UDFParser.color(a["foreground"]) ?? NSColor.black,
        ]
        if a["underline"] == "true" { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        if a["strikethrough"] == "true" { attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
        if a["superscript"] == "true" { attrs[.superscript] = 1 }
        if a["subscript"] == "true" { attrs[.superscript] = -1 }
        return attrs
    }

    private static func paragraph(_ p: UDFParagraph, doc: UDFDocument, width: CGFloat, counter: inout ListCounter, block: NSTextBlock?) -> NSAttributedString {
        let style = paragraphStyle(p.attrs, width: width)
        if let block { style.textBlocks = [block] }
        let out = NSMutableAttributedString()
        if let label = listLabel(p.attrs, counter: &counter) {
            let tabLocation = max(style.headIndent, style.firstLineHeadIndent + 18)
            style.tabStops = [NSTextTab(textAlignment: .left, location: tabLocation)] + style.tabStops
            out.append(NSAttributedString(string: label + "\t", attributes: runAttributes(p.attrs, paragraphStyle: style)))
        }
        var lastAttrs = runAttributes(p.attrs, paragraphStyle: style)
        for run in p.runs {
            let attrs = runAttributes(run.attrs, paragraphStyle: style)
            lastAttrs = attrs
            switch run.kind {
            case .image(let data, let size):
                guard let image = NSImage(data: data) else { continue }
                let attachment = NSTextAttachment()
                attachment.image = image
                let w = size.width > 0 ? size.width : image.size.width
                let h = size.height > 0 ? size.height : image.size.height
                attachment.bounds = CGRect(x: 0, y: 0, width: w, height: h)
                let piece = NSMutableAttributedString(attachment: attachment)
                piece.addAttributes([.paragraphStyle: style], range: NSRange(location: 0, length: piece.length))
                out.append(piece)
            case .text:
                let start = min(run.range.location, doc.text.length)
                let range = NSRange(location: start, length: min(run.range.length, doc.text.length - start))
                let text = cleaned(doc.text.substring(with: range))
                if !text.isEmpty { out.append(NSAttributedString(string: text, attributes: attrs)) }
            }
        }
        if !out.string.hasSuffix("\n") {
            out.append(NSAttributedString(string: "\n", attributes: lastAttrs))
        }
        out.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: out.length))
        return out
    }

    private static func table(_ t: UDFTable, doc: UDFDocument, width: CGFloat, counter: inout ListCounter) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let columns = max(1, t.rows.map(\.count).max() ?? 1)
        let table = NSTextTable()
        table.numberOfColumns = columns
        table.layoutAlgorithm = .fixedLayoutAlgorithm
        table.collapsesBorders = true

        var spans = (t.attrs["columnSpans"] ?? "").split(separator: ",").compactMap { Double($0).map { CGFloat($0) } }
        if spans.count != columns { spans = Array(repeating: width / CGFloat(columns), count: columns) }
        let total = spans.reduce(0, +)
        if total > width { spans = spans.map { $0 * width / total } }
        let bordered = (t.attrs["border"] ?? "borderNone") != "borderNone"

        for (r, row) in t.rows.enumerated() {
            for c in 0..<columns {
                let block = NSTextTableBlock(table: table, startingRow: r, rowSpan: 1, startingColumn: c, columnSpan: 1)
                block.setValue(spans[c], type: .absoluteValueType, for: .width)
                block.setWidth(1, type: .absoluteValueType, for: .padding)
                if bordered {
                    block.setWidth(0.5, type: .absoluteValueType, for: .border)
                    block.setBorderColor(.black)
                }
                let paragraphs = c < row.count ? row[c] : []
                if paragraphs.isEmpty {
                    let style = NSMutableParagraphStyle()
                    style.textBlocks = [block]
                    out.append(NSAttributedString(string: "\n", attributes: [.paragraphStyle: style, .font: font([:])]))
                }
                for p in paragraphs {
                    out.append(paragraph(p, doc: doc, width: spans[c], counter: &counter, block: block))
                }
            }
        }
        return out
    }
}

// MARK: - Rendering

enum UDFRenderer {
    private final class Flow {
        let storage: NSTextStorage
        let layout = NSLayoutManager()
        init(_ text: NSAttributedString) {
            storage = NSTextStorage(attributedString: text)
            storage.addLayoutManager(layout)
            layout.allowsNonContiguousLayout = false
        }
        func container(width: CGFloat, height: CGFloat) -> NSTextContainer {
            let container = NSTextContainer(size: CGSize(width: width, height: height))
            container.lineFragmentPadding = 0
            layout.addTextContainer(container)
            layout.ensureLayout(for: container)
            return container
        }
    }

    /// Renders one UDF file to a PDF. Returns the page count.
    @discardableResult
    static func render(_ url: URL, to output: URL) throws -> Int {
        let doc = try UDFParser.parse(url)
        let page = doc.pageSize
        let m = doc.margins
        let width = page.width - m.left - m.right
        guard width > 50 else { throw AppError.message("UDF sayfa kenar boşlukları geçersiz.") }

        let header = Flow(UDFTypesetter.attributed(doc.header, doc: doc, width: width))
        let headerBox = header.container(width: width, height: .greatestFiniteMagnitude)
        let headerHeight = doc.header.isEmpty ? 0 : header.layout.usedRect(for: headerBox).height
        let footer = Flow(UDFTypesetter.attributed(doc.footer, doc: doc, width: width))
        let footerBox = footer.container(width: width, height: .greatestFiniteMagnitude)
        let footerHeight = doc.footer.isEmpty ? 0 : footer.layout.usedRect(for: footerBox).height

        let top = max(m.top, doc.headerOffset + headerHeight)
        let bottom = max(m.bottom, doc.footerOffset + footerHeight)
        let bodyHeight = page.height - top - bottom
        guard bodyHeight > 40 else { throw AppError.message("UDF üst/alt bilgi alanları sayfaya sığmıyor.") }

        let body = Flow(UDFTypesetter.attributed(doc.body, doc: doc, width: width))
        var pages: [NSTextContainer] = []
        repeat {
            let container = body.container(width: width, height: bodyHeight)
            pages.append(container)
            if pages.count > 1, body.layout.glyphRange(for: container).length == 0 { break }
        } while NSMaxRange(body.layout.glyphRange(for: pages.last!)) < body.layout.numberOfGlyphs && pages.count < 5000

        // Trailing pages holding nothing but empty lines are dropped.
        let text = body.storage.string as NSString
        while pages.count > 1 {
            let glyphs = body.layout.glyphRange(for: pages.last!)
            let chars = body.layout.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
            if text.substring(with: chars).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { pages.removeLast() } else { break }
        }

        var box = CGRect(origin: .zero, size: page)
        let info: [CFString: Any] = [kCGPDFContextCreator: PDFEngine.creator, kCGPDFContextTitle: Output.baseName(url)]
        guard let ctx = CGContext(output as CFURL, mediaBox: &box, info as CFDictionary) else {
            throw AppError.cannotWrite(output.lastPathComponent)
        }
        let total = pages.count
        for (index, container) in pages.enumerated() {
            ctx.beginPDFPage(nil)
            if let bg = doc.background {
                let i = bg.insets
                ctx.interpolationQuality = .high
                ctx.draw(bg.image, in: CGRect(x: i.left, y: i.bottom, width: page.width - i.left - i.right, height: page.height - i.top - i.bottom))
            }
            ctx.saveGState()
            ctx.translateBy(x: 0, y: page.height)
            ctx.scaleBy(x: 1, y: -1)
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)

            draw(header.layout, header.layout.glyphRange(for: headerBox), at: CGPoint(x: m.left, y: doc.headerOffset))
            let glyphs = body.layout.glyphRange(for: container)
            draw(body.layout, glyphs, at: CGPoint(x: m.left, y: top))
            draw(footer.layout, footer.layout.glyphRange(for: footerBox), at: CGPoint(x: m.left, y: page.height - doc.footerOffset - footerHeight))

            if let number = doc.headerPageNumber {
                drawPageNumber(number, page: index, total: total, rect: CGRect(x: m.left, y: doc.headerOffset, width: width, height: 20), top: true)
            }
            if let number = doc.footerPageNumber {
                drawPageNumber(number, page: index, total: total, rect: CGRect(x: m.left, y: page.height - doc.footerOffset - 20, width: width, height: 20), top: false)
            }

            NSGraphicsContext.restoreGraphicsState()
            ctx.restoreGState()
            ctx.endPDFPage()
        }
        ctx.closePDF()
        return total
    }

    private static func draw(_ layout: NSLayoutManager, _ glyphs: NSRange, at origin: CGPoint) {
        guard glyphs.length > 0 else { return }
        layout.drawBackground(forGlyphRange: glyphs, at: origin)
        layout.drawGlyphs(forGlyphRange: glyphs, at: origin)
    }

    private static func drawPageNumber(_ n: UDFPageNumber, page: Int, total: Int, rect: CGRect, top: Bool) {
        let current = n.start + page
        var text = n.leading + "\(current)"
        if n.showTotal { text += n.separator + "\(n.start + total - 1)" }
        let style = NSMutableParagraphStyle()
        style.alignment = n.alignment
        let attrs: [NSAttributedString.Key: Any] = [.font: n.font, .foregroundColor: n.color, .paragraphStyle: style]
        let height = (text as NSString).size(withAttributes: attrs).height
        let y = top ? rect.minY : rect.maxY - height
        (text as NSString).draw(in: CGRect(x: rect.minX, y: y, width: rect.width, height: height), withAttributes: attrs)
    }

    /// Converts several UDF files; with `merge` the pages go into one PDF in the given order.
    static func convert(_ urls: [URL], merge: Bool, mergedName: String, progress: ProgressReporter) throws -> [ResultItem] {
        let temp = try Output.tempDirectory()
        defer { try? FileManager.default.removeItem(at: temp) }
        var results: [ResultItem] = []
        var rendered: [URL] = []
        for (i, url) in urls.enumerated() {
            progress(Double(i) / Double(urls.count), "Dönüştürülüyor: \(url.lastPathComponent)")
            do {
                if merge {
                    let out = temp.appendingPathComponent("\(i).pdf")
                    try render(url, to: out)
                    rendered.append(out)
                } else {
                    let out = Output.uniqueFile(in: Output.directory(for: url), base: Output.baseName(url), ext: "pdf")
                    let pages = try render(url, to: out)
                    results.append(ResultItem(title: out.lastPathComponent, detail: "\(pages) sayfa · \(formatBytes(fileSize(out)))", url: out))
                }
            } catch {
                results.append(ResultItem(title: url.lastPathComponent, detail: error.localizedDescription, success: false))
            }
        }
        if merge, let first = urls.first {
            guard !rendered.isEmpty else { return results }
            let name = mergedName.trimmingCharacters(in: .whitespaces).isEmpty ? Output.baseName(first) : mergedName
            let out = Output.uniqueFile(in: Output.directory(for: first), base: name, ext: "pdf")
            progress(0.95, "Birleştiriliyor…")
            let pages = try PDFEngine.merge(rendered, to: out, progress: ProgressReporter { _, _ in })
            results.insert(ResultItem(title: out.lastPathComponent,
                                      detail: "\(rendered.count) UDF · \(pages) sayfa · \(formatBytes(fileSize(out)))", url: out), at: 0)
        }
        return results
    }
}
