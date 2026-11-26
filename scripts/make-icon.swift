// Draws the 1024×1024 icon: a rounded square with a dark aurora gradient, a flash drive and a speed arc.
// Run: swift scripts/make-icon.swift assets/icon.png
import AppKit

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext
let space = CGColorSpaceCreateDeviceRGB()
func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor { CGColor(red: r / 255, green: g / 255, blue: b / 255, alpha: a) }

// macOS icon grid: 824 of 1024
let inset: CGFloat = 100
let rect = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let shape = CGPath(roundedRect: rect, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: rgb(0, 0, 0, 0.35))
ctx.addPath(shape); ctx.setFillColor(rgb(10, 11, 20)); ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(shape); ctx.clip()
let bg = CGGradient(colorsSpace: space, colors: [rgb(16, 18, 34), rgb(8, 9, 16)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(bg, start: CGPoint(x: 0, y: size), end: CGPoint(x: size, y: 0), options: [])
// aurora glows
for (c, x, y, r) in [(rgb(79, 227, 193, 0.55), 280.0, 760.0, 420.0), (rgb(139, 140, 255, 0.55), 780.0, 600.0, 460.0),
                     (rgb(255, 111, 181, 0.40), 560.0, 180.0, 420.0)] {
    let g = CGGradient(colorsSpace: space, colors: [c, rgb(0, 0, 0, 0)] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(g, startCenter: CGPoint(x: x, y: y), startRadius: 0, endCenter: CGPoint(x: x, y: y),
                           endRadius: CGFloat(r), options: [])
}
ctx.restoreGState()

// speed arc
let center = CGPoint(x: 512, y: 470)
ctx.saveGState()
ctx.setLineWidth(46); ctx.setLineCap(.round)
ctx.setStrokeColor(rgb(255, 255, 255, 0.10))
ctx.addArc(center: center, radius: 300, startAngle: .pi * 1.15, endAngle: -.pi * 0.15, clockwise: true); ctx.strokePath()
ctx.addArc(center: center, radius: 300, startAngle: .pi * 1.15, endAngle: .pi * 0.18, clockwise: true)
ctx.replacePathWithStrokedPath(); ctx.clip()
let arc = CGGradient(colorsSpace: space, colors: [rgb(79, 227, 193), rgb(139, 140, 255), rgb(255, 111, 181)] as CFArray,
                     locations: [0, 0.55, 1])!
ctx.drawLinearGradient(arc, start: CGPoint(x: 200, y: 400), end: CGPoint(x: 820, y: 600), options: [])
ctx.restoreGState()

// flash drive
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 30, color: rgb(0, 0, 0, 0.45))
let body = CGPath(roundedRect: CGRect(x: 412, y: 250, width: 200, height: 300), cornerWidth: 46, cornerHeight: 46, transform: nil)
ctx.addPath(body); ctx.setFillColor(rgb(244, 246, 255)); ctx.fillPath()
ctx.restoreGState()
let plug = CGPath(roundedRect: CGRect(x: 452, y: 540, width: 120, height: 130), cornerWidth: 16, cornerHeight: 16, transform: nil)
ctx.addPath(plug); ctx.setStrokeColor(rgb(244, 246, 255)); ctx.setLineWidth(22); ctx.strokePath()
ctx.setFillColor(rgb(244, 246, 255))
ctx.fill(CGRect(x: 482, y: 600, width: 22, height: 30)); ctx.fill(CGRect(x: 520, y: 600, width: 22, height: 30))
// check mark on the body
ctx.setStrokeColor(rgb(16, 170, 120)); ctx.setLineWidth(30); ctx.setLineCap(.round); ctx.setLineJoin(.round)
ctx.move(to: CGPoint(x: 462, y: 400)); ctx.addLine(to: CGPoint(x: 500, y: 360)); ctx.addLine(to: CGPoint(x: 566, y: 440)); ctx.strokePath()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
