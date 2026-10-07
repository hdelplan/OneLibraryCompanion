import Cocoa
let image = NSImage(size: NSSize(width: 1024, height: 1024))
image.lockFocus()
NSColor(calibratedRed: 0.06, green: 0.09, blue: 0.12, alpha: 1).setFill()
// iPadOS supplies the corner mask; its source icon needs an opaque full background.
if CommandLine.arguments.contains("--ipad") {
    NSBezierPath(rect: NSRect(x: 0, y: 0, width: 1024, height: 1024)).fill()
} else {
    NSBezierPath(roundedRect: NSRect(x: 32, y: 32, width: 960, height: 960), xRadius: 212, yRadius: 212).fill()
}
let wave = NSBezierPath()
wave.move(to: NSPoint(x: 152, y: 676))
for (x, y) in [(244,676),(288,792),(352,552),(416,736),(476,676),(872,676)] { wave.line(to: NSPoint(x:x,y:y)) }
wave.lineWidth = 36
wave.lineCapStyle = .round
wave.lineJoinStyle = .round
NSColor(calibratedRed: 0.93, green: 0.67, blue: 0.33, alpha: 1).setStroke()
wave.stroke()
let text = "OLC" as NSString
let attributes: [NSAttributedString.Key:Any] = [.font:NSFont.boldSystemFont(ofSize:280), .foregroundColor:NSColor(calibratedWhite:0.95,alpha:1)]
let size = text.size(withAttributes:attributes)
text.draw(at:NSPoint(x:(1024-size.width)/2,y:220),withAttributes:attributes)
image.unlockFocus()
let bitmap = NSBitmapImageRep(data:image.tiffRepresentation!)!
var output = bitmap
if CommandLine.arguments.contains("--ipad") {
    let context = CGContext(data: nil, width: 1024, height: 1024,
                            bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    context.draw(bitmap.cgImage!, in: CGRect(x: 0, y: 0, width: 1024, height: 1024))
    output = NSBitmapImageRep(cgImage: context.makeImage()!)
}
try output.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:CommandLine.arguments[1]))
