// Draws the app icon, 1024 × 1024: a letter under a crescent moon, night
// blue, in the GUI's colours. usage: swift icon.swift <out.png>

import AppKit

let size = 1024
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat(hex >> 16 & 0xFF) / 255, green: CGFloat(hex >> 8 & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

// macOS's icon grid: an 824-point rounded square, with room for its shadow.
let tile = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824), cornerWidth: 185, cornerHeight: 185, transform: nil)
context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0x000000, 0.35))
context.addPath(tile)
context.setFillColor(color(0x0F1522))
context.fillPath()
context.restoreGState()
context.saveGState()
context.addPath(tile)
context.clip()
let night = CGGradient(colorsSpace: space, colors: [color(0x1C2740), color(0x0A0F19)] as CFArray, locations: [0, 1])!
context.drawLinearGradient(night, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

// The letter, with a gap cut round the moon.
let moon = CGPoint(x: 712, y: 724)
context.beginTransparencyLayer(auxiliaryInfo: nil)
context.setStrokeColor(color(0xEEF2F9))
context.setLineWidth(40)
context.setLineJoin(.round)
context.setLineCap(.round)
context.addPath(CGPath(roundedRect: CGRect(x: 262, y: 300, width: 500, height: 340), cornerWidth: 44, cornerHeight: 44, transform: nil))
context.strokePath()
context.move(to: CGPoint(x: 290, y: 612))
context.addLine(to: CGPoint(x: 512, y: 452))
context.addLine(to: CGPoint(x: 734, y: 612))
context.strokePath()
context.setBlendMode(.clear)
context.fillEllipse(in: CGRect(x: moon.x - 142, y: moon.y - 142, width: 284, height: 284))
context.endTransparencyLayer()

// The crescent: a disc less a disc, in the GUI's teal.
context.beginTransparencyLayer(auxiliaryInfo: nil)
context.setFillColor(color(0x5CC8C2))
context.fillEllipse(in: CGRect(x: moon.x - 112, y: moon.y - 112, width: 224, height: 224))
context.setBlendMode(.clear)
context.fillEllipse(in: CGRect(x: moon.x - 52, y: moon.y - 40, width: 200, height: 200))
context.endTransparencyLayer()
context.restoreGState()

let png = NSBitmapImageRep(cgImage: context.makeImage()!).representation(using: .png, properties: [:])!
try png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
