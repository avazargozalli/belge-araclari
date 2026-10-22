// Draws the app icon (1024×1024 PNG) with Core Graphics. Usage: swift make_icon.swift <output.png>
import AppKit

let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"
let size: CGFloat = 1024
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext

// Squircle background inside the standard macOS icon grid (100 px margin).
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor.black.withAlphaComponent(0.28).cgColor)
ctx.addPath(tilePath)
ctx.setFillColor(NSColor(red: 0.04, green: 0.45, blue: 0.36, alpha: 1).cgColor)
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(tilePath)
ctx.clip()
let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [
    NSColor(red: 0.13, green: 0.80, blue: 0.60, alpha: 1).cgColor,
    NSColor(red: 0.03, green: 0.47, blue: 0.43, alpha: 1).cgColor
] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(gradient, start: CGPoint(x: 200, y: 924), end: CGPoint(x: 824, y: 100), options: [])
// Soft highlight at the top.
let glow = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [
    NSColor.white.withAlphaComponent(0.22).cgColor, NSColor.white.withAlphaComponent(0).cgColor
] as CFArray, locations: [0, 1])!
ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 360, y: 900), startRadius: 0,
                       endCenter: CGPoint(x: 360, y: 900), endRadius: 520, options: [])
ctx.restoreGState()

// Document with folded corner.
let doc = CGRect(x: 312, y: 228, width: 400, height: 520)
let fold: CGFloat = 112
let page = CGMutablePath()
page.move(to: CGPoint(x: doc.minX + 36, y: doc.minY))
page.addLine(to: CGPoint(x: doc.maxX - 36, y: doc.minY))
page.addQuadCurve(to: CGPoint(x: doc.maxX, y: doc.minY + 36), control: CGPoint(x: doc.maxX, y: doc.minY))
page.addLine(to: CGPoint(x: doc.maxX, y: doc.maxY - fold))
page.addLine(to: CGPoint(x: doc.maxX - fold, y: doc.maxY))
page.addLine(to: CGPoint(x: doc.minX + 36, y: doc.maxY))
page.addQuadCurve(to: CGPoint(x: doc.minX, y: doc.maxY - 36), control: CGPoint(x: doc.minX, y: doc.maxY))
page.addLine(to: CGPoint(x: doc.minX, y: doc.minY + 36))
page.addQuadCurve(to: CGPoint(x: doc.minX + 36, y: doc.minY), control: CGPoint(x: doc.minX, y: doc.minY))
page.closeSubpath()

ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -18), blur: 36, color: NSColor.black.withAlphaComponent(0.3).cgColor)
ctx.addPath(page)
ctx.setFillColor(NSColor.white.cgColor)
ctx.fillPath()
ctx.restoreGState()

let corner = CGMutablePath()
corner.move(to: CGPoint(x: doc.maxX - fold, y: doc.maxY))
corner.addLine(to: CGPoint(x: doc.maxX - fold, y: doc.maxY - fold + 24))
corner.addQuadCurve(to: CGPoint(x: doc.maxX - fold + 24, y: doc.maxY - fold), control: CGPoint(x: doc.maxX - fold, y: doc.maxY - fold))
corner.addLine(to: CGPoint(x: doc.maxX, y: doc.maxY - fold))
corner.closeSubpath()
ctx.addPath(corner)
ctx.setFillColor(NSColor(red: 0.80, green: 0.93, blue: 0.89, alpha: 1).cgColor)
ctx.fillPath()

// Text lines.
ctx.setFillColor(NSColor(red: 0.78, green: 0.86, blue: 0.84, alpha: 1).cgColor)
for (i, width) in [230.0, 300, 260].enumerated() {
    let y = doc.maxY - 150 - CGFloat(i) * 46
    ctx.addPath(CGPath(roundedRect: CGRect(x: doc.minX + 56, y: y, width: width, height: 20), cornerWidth: 10, cornerHeight: 10, transform: nil))
    ctx.fillPath()
}

// "PDF" label.
let label = "PDF" as NSString
let font = NSFont.systemFont(ofSize: 132, weight: .heavy)
let attributes: [NSAttributedString.Key: Any] = [
    .font: font,
    .foregroundColor: NSColor(red: 0.03, green: 0.52, blue: 0.42, alpha: 1),
    .kern: -2
]
let textSize = label.size(withAttributes: attributes)
label.draw(at: CGPoint(x: doc.midX - textSize.width / 2, y: doc.minY + 44), withAttributes: attributes)

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("icon → \(output)")
