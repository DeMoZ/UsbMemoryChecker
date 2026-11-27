// Draws the app icon (a USB stick with a check mark on a dark tile) natively for every size it ships in:
// nothing is shrunk from a large master.
//   swift scripts/make-icon.swift <outdir>
//       writes mac-*.png (icns, macOS icon grid), win-*.png (full canvas) and icon.ico
//   swift scripts/make-icon.swift --sheet <icon.ico> <AppIcon.icns> <out.png>
//       reads the packed files back and draws every image at 1x on light and dark backgrounds
// scripts/make-icon.sh runs both and fills assets/.
import AppKit
import ImageIO
import UniformTypeIdentifiers

let macSizes = [16, 32, 64, 128, 256, 512, 1024]
let winSizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
let space = CGColorSpace(name: CGColorSpace.sRGB)!

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func rgb(_ hex: Int, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [CGFloat((hex >> 16) & 255) / 255, CGFloat((hex >> 8) & 255) / 255,
                                            CGFloat(hex & 255) / 255, alpha])!
}

let tileTop = 0x1A1F2B, tileBottom = 0x0C0F15, tileFlat = 0x12161F
let plugColor = 0xEDF0F6, bodyColor = 0x5EE2E8, checkColor = 0x0C0F15, white = 0xFFFFFF

/// A bitmap context with the origin at the top left (y down).
func makeContext(_ width: Int, _ height: Int) -> CGContext {
    let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x: 0, y: CGFloat(height))
    ctx.scaleBy(x: 1, y: -1)
    return ctx
}

func rounded(_ r: CGRect, _ radius: CGFloat) -> CGPath {
    let c = max(0, min(radius, min(r.width, r.height) / 2))
    return CGPath(roundedRect: r, cornerWidth: c, cornerHeight: c, transform: nil)
}

func fill(_ ctx: CGContext, _ path: CGPath, _ color: CGColor) {
    ctx.addPath(path)
    ctx.setFillColor(color)
    ctx.fillPath()
}

/// The parts of the stick in pixels, relative to the tile's top left corner.
struct Stick {
    var plug: CGRect
    var plugRadius: CGFloat
    var holes: [CGRect]
    var body: CGRect
    var bodyRadius: CGFloat
    var check: [CGPoint]
    var checkWidth: CGFloat
}

/// Small sizes (canvas up to 48 px): a 16-unit grid scaled to the tile, every edge on a whole pixel.
/// `n` is the canvas size, `t` the tile size.
func smallStick(n: Int, t: Int) -> Stick {
    let s = CGFloat(t) / 16
    func px(_ v: CGFloat) -> CGFloat { (v * s).rounded() }
    // symmetric parts: the right edge mirrors the left one
    func centred(_ x0: CGFloat, _ y0: CGFloat, _ y1: CGFloat) -> CGRect {
        CGRect(x: px(x0), y: px(y0), width: CGFloat(t) - 2 * px(x0), height: px(y1) - px(y0))
    }
    var plug = centred(6, 2, 6)
    var body = centred(4, 6, 14)
    if t == 12 {
        // the 16 px macOS icon: a shorter plug and a body 1 px larger to each side and upwards, so the check fits
        plug = CGRect(x: 4, y: 1, width: 4, height: 3)
        body = CGRect(x: 2, y: 4, width: 8, height: 7)
    }
    var holes: [CGRect] = []
    if n >= 24 {
        // two equal holes with at least 1 px of plug between and around them
        let w = max(1, (plug.width / 4).rounded(.down)), inset = max(1, ((plug.width - 2 * w) / 3).rounded())
        let y = plug.minY + max(1, px(1.25)), h = max(2, px(1.25))
        holes = [CGRect(x: plug.minX + inset, y: y, width: w, height: h),
                 CGRect(x: plug.maxX - inset - w, y: y, width: w, height: h)]
    }
    let width = max(2, (CGFloat(t) / 8).rounded())
    // odd stroke widths run through pixel centres, even ones along pixel edges
    let half: CGFloat = Int(width) % 2 == 1 ? 0.5 : 0
    func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
        CGPoint(x: (x * s - half).rounded() + half, y: (y * s - half).rounded() + half)
    }
    var check = [point(5.5, 10), point(7.5, 12), point(10.5, 8.5)]
    // hand-tuned by about a pixel (design D11), keyed by the tile size: cyan stays visible around the check, so the body keeps
    // its outline (12 and 26 are the 16 and 32 px macOS icons)
    let tuned: [Int: [(CGFloat, CGFloat)]] = [
        12: [(4, 7), (6, 9), (8.5, 6.5)],
        16: [(6, 10), (8, 12), (10.5, 9.5)],
        20: [(7.5, 12.5), (9.5, 14.5), (12.5, 11.5)],
        24: [(8.5, 14.5), (10.5, 16.5), (15.5, 11.5)],
        26: [(10.5, 16.5), (12.5, 18.5), (16.5, 14.5)],
        32: [(12, 19), (15, 22), (21, 16)],
    ]
    if let points = tuned[t] { check = points.map { CGPoint(x: $0.0, y: $0.1) } }
    return Stick(plug: plug, plugRadius: n >= 24 ? 1 : 0, holes: holes, body: body, bodyRadius: 1.5 * s,
                 check: check, checkWidth: width)
}

/// Large sizes (64 px and up): the same parts in 1024 tile units, the stick 60% of the tile height.
func largeStick(t: CGFloat) -> Stick {
    let u = t / 1024
    func r(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> CGRect {
        CGRect(x: x * u, y: y * u, width: w * u, height: h * u)
    }
    return Stick(plug: r(410, 205, 204, 230), plugRadius: 20 * u,
                 holes: [r(448, 269, 40, 66), r(536, 269, 40, 66)],
                 body: r(307, 410, 410, 409), bodyRadius: 77 * u,
                 check: [CGPoint(x: 394 * u, y: 614 * u), CGPoint(x: 480 * u, y: 700 * u),
                         CGPoint(x: 636 * u, y: 536 * u)],
                 checkWidth: 62 * u)
}

/// One drawing for every variant: a canvas of `n` px with a tile inset by `inset` px.
func drawIcon(n: Int, inset: Int, radius: CGFloat, shadow: Bool) -> CGContext {
    let ctx = makeContext(n, n)
    let t = n - 2 * inset
    let small = n <= 48
    let tile = CGRect(x: inset, y: inset, width: t, height: t)
    let shape = rounded(tile, radius)

    if shadow {
        // shadow offset and blur are in base space (y up), not affected by the flip
        let k = CGFloat(n) / 1024
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -12 * k), blur: 28 * k, color: rgb(0, 0.35))
        fill(ctx, shape, rgb(tileBottom))
        ctx.restoreGState()
    }

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    if n >= 24 {
        let gradient = CGGradient(colorsSpace: space, colors: [rgb(tileTop), rgb(tileBottom)] as CFArray,
                                  locations: [0, 1])!
        ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: tile.minY), end: CGPoint(x: 0, y: tile.maxY),
                               options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    } else {
        ctx.setFillColor(rgb(tileFlat))
        ctx.fill(tile)
    }
    // 1 px inner edge, all round: keeps the tile visible on a dark taskbar
    ctx.addPath(rounded(tile.insetBy(dx: 0.5, dy: 0.5), radius - 0.5))
    ctx.setStrokeColor(rgb(white, 0.16))
    ctx.setLineWidth(1)
    ctx.strokePath()
    if n >= 24 {
        // 1 px top highlight: the part of the tile that is not covered by the tile moved down by 1 px
        ctx.saveGState()
        ctx.addRect(CGRect(x: 0, y: 0, width: n, height: n))
        ctx.addPath(rounded(tile.offsetBy(dx: 0, dy: 1), radius))
        ctx.clip(using: .evenOdd)
        fill(ctx, shape, rgb(white, 0.10))
        ctx.restoreGState()
    }
    ctx.restoreGState()

    ctx.saveGState()
    ctx.translateBy(x: tile.minX, y: tile.minY)
    if !small {
        // speed arc behind the stick: a 270 degree gauge, open at the bottom, 70% filled
        let u = CGFloat(t) / 1024
        let centre = CGPoint(x: 512 * u, y: 512 * u)
        let start = CGFloat.pi * 0.75, sweep = CGFloat.pi * 1.5
        ctx.setLineWidth(56 * u)
        ctx.setLineCap(.round)
        for (to, color) in [(CGFloat(1), rgb(white, 0.10)), (CGFloat(0.7), rgb(bodyColor, 0.70))] {
            // angles grow clockwise on screen because y points down
            ctx.addArc(center: centre, radius: 364 * u, startAngle: start, endAngle: start + sweep * to,
                       clockwise: false)
            ctx.setStrokeColor(color)
            ctx.strokePath()
        }
    }
    let stick = small ? smallStick(n: n, t: t) : largeStick(t: CGFloat(t))
    // the plug is rounded at the top only: its lower end runs under the body
    ctx.saveGState()
    ctx.clip(to: stick.plug)
    var plug = stick.plug
    plug.size.height += stick.plugRadius * 2
    fill(ctx, rounded(plug, stick.plugRadius), rgb(plugColor))
    ctx.restoreGState()
    for hole in stick.holes { fill(ctx, CGPath(rect: hole, transform: nil), rgb(tileFlat)) }
    fill(ctx, rounded(stick.body, stick.bodyRadius), rgb(bodyColor))
    ctx.addLines(between: stick.check)
    ctx.setStrokeColor(rgb(checkColor))
    ctx.setLineWidth(stick.checkWidth)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.strokePath()
    ctx.restoreGState()
    return ctx
}

/// Windows: the tile fills the canvas.
func winIcon(_ n: Int) -> CGContext {
    drawIcon(n: n, inset: n <= 48 ? 0 : Int((Double(n) / 64).rounded()), radius: CGFloat(n) * 0.22, shadow: false)
}

/// macOS: the Big Sur icon grid (tile 824 of 1024).
func macIcon(_ n: Int) -> CGContext {
    drawIcon(n: n, inset: Int((Double(n) * 100 / 1024).rounded()), radius: CGFloat(n) * 185 / 1024,
             shadow: n >= 128)
}

func pngData(_ image: CGImage) -> Data {
    let data = NSMutableData()
    let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) { fail("cannot encode a PNG") }
    return data as Data
}

extension Data {
    mutating func le16(_ v: Int) { append(contentsOf: [UInt8(v & 255), UInt8((v >> 8) & 255)]) }
    mutating func le32(_ v: Int) { le16(v & 0xFFFF); le16((v >> 16) & 0xFFFF) }
}

/// A 32-bit BGRA DIB as an .ico entry: bottom-up, straight (not premultiplied) alpha, then the 1-bit AND mask.
func dibData(_ ctx: CGContext) -> Data {
    let n = ctx.width
    let pixels = ctx.data!.bindMemory(to: UInt8.self, capacity: ctx.bytesPerRow * n)
    let maskRow = (n + 31) / 32 * 4
    var colour = Data()
    var mask = Data()
    for y in stride(from: n - 1, through: 0, by: -1) {
        var bits = [UInt8](repeating: 0, count: maskRow)
        for x in 0..<n {
            let p = y * ctx.bytesPerRow + x * 4
            let a = Int(pixels[p + 3])
            func straight(_ c: UInt8) -> UInt8 { a == 0 ? 0 : UInt8(min(255, (Int(c) * 255 + a / 2) / a)) }
            colour.append(contentsOf: [straight(pixels[p + 2]), straight(pixels[p + 1]), straight(pixels[p]), UInt8(a)])
            if a == 0 { bits[x / 8] |= 0x80 >> UInt8(x % 8) }
        }
        mask.append(contentsOf: bits)
    }
    var out = Data()
    out.le32(40)          // BITMAPINFOHEADER
    out.le32(n)
    out.le32(2 * n)       // colour and mask
    out.le16(1)
    out.le16(32)
    out.le32(0)           // BI_RGB
    out.le32(colour.count + mask.count)
    for _ in 0..<4 { out.le32(0) }
    return out + colour + mask
}

func icoData(_ entries: [(size: Int, data: Data)]) -> Data {
    var out = Data()
    out.le16(0)
    out.le16(1)
    out.le16(entries.count)
    var offset = 6 + 16 * entries.count
    for e in entries {
        out.append(contentsOf: [UInt8(e.size % 256), UInt8(e.size % 256), 0, 0])
        out.le16(1)
        out.le16(32)
        out.le32(e.data.count)
        out.le32(offset)
        offset += e.data.count
    }
    for e in entries { out.append(e.data) }
    return out
}

func drawAll(into dir: URL) throws {
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    for n in macSizes {
        try pngData(macIcon(n).makeImage()!).write(to: dir.appendingPathComponent("mac-\(n).png"))
    }
    var entries: [(size: Int, data: Data)] = []
    for n in winSizes {
        let ctx = winIcon(n)
        let png = pngData(ctx.makeImage()!)
        try png.write(to: dir.appendingPathComponent("win-\(n).png"))
        entries.append((n, n <= 48 ? dibData(ctx) : png))
    }
    try icoData(entries).write(to: dir.appendingPathComponent("icon.ico"))
    print("wrote \(macSizes.count) mac and \(winSizes.count) win images and icon.ico to \(dir.path)")
}

/// Every image of a packed icon file, smallest first.
func images(of path: String) -> [CGImage] {
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil) else {
        fail("cannot read \(path)")
    }
    let list = (0..<CGImageSourceGetCount(source)).compactMap { CGImageSourceCreateImageAtIndex(source, $0, nil) }
    if list.isEmpty { fail("no images in \(path)") }
    return list.sorted { $0.width < $1.width }
}

func contactSheet(ico: String, icns: String, out: String) throws {
    let backgrounds = [0xFFFFFF, 0xF3F3F3, 0x202020, 0x000000]
    let zoomed = images(of: ico).filter { [16, 24, 32].contains($0.width) }
    // one row per image: (label, image, scale)
    var rows: [(String, CGImage, Int)] = zoomed.map { ("ico \($0.width) at 4x", $0, 4) }
    for (name, path) in [("ico", ico), ("icns", icns)] {
        rows += images(of: path).map { ("\(name) \($0.width) px", $0, 1) }
    }
    // small images come first in narrow columns, so they can be judged without scrolling sideways
    let small = rows.filter { $0.1.width * $0.2 <= 128 }
    let large = rows.filter { $0.1.width * $0.2 > 128 }
    rows = small + large
    let pad = 12, labelWidth = 130
    func cellWidth(_ size: Int) -> Int { max(160, size + 2 * pad) }
    let largest = rows.map { $0.1.width * $0.2 }.max()!
    let width = labelWidth + cellWidth(largest) * backgrounds.count
    let height = rows.reduce(0) { $0 + $1.1.height * $1.2 + 2 * pad }
    let ctx = makeContext(width, height)
    ctx.setFillColor(rgb(0x9AA0AA))
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    ctx.interpolationQuality = .none
    var y = 0
    for (label, image, scale) in rows {
        let w = image.width * scale, h = image.height * scale
        let rowHeight = h + 2 * pad
        let cell = cellWidth(w <= 128 ? 0 : largest)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
        (label as NSString).draw(at: NSPoint(x: 8, y: y + rowHeight / 2 - 8),
                                 withAttributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.black])
        NSGraphicsContext.restoreGraphicsState()
        for (i, background) in backgrounds.enumerated() {
            let x = labelWidth + i * cell
            ctx.setFillColor(rgb(background))
            ctx.fill(CGRect(x: x, y: y, width: cell, height: rowHeight))
            // images are drawn y up: flip around the image's own box
            ctx.saveGState()
            ctx.translateBy(x: CGFloat(x + (cell - w) / 2), y: CGFloat(y + pad + h))
            ctx.scaleBy(x: 1, y: -1)
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            ctx.restoreGState()
        }
        y += rowHeight
    }
    let url = URL(fileURLWithPath: out)
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try pngData(ctx.makeImage()!).write(to: url)
    print("wrote \(out)")
}

let args = Array(CommandLine.arguments.dropFirst())
if args.count == 4 && args[0] == "--sheet" {
    try contactSheet(ico: args[1], icns: args[2], out: args[3])
} else if args.count == 1 && !args[0].hasPrefix("-") {
    try drawAll(into: URL(fileURLWithPath: args[0]))
} else {
    fail("usage: make-icon.swift <outdir> | make-icon.swift --sheet <icon.ico> <AppIcon.icns> <out.png>")
}
