import CoreGraphics
import Foundation
import SwiftUI

/// What a phone shows while watching a computer's screen.
enum RemoteScreenState: Equatable, Sendable {
    /// Waiting for the first pixels.
    case opening
    case watching
    /// The computer stopped sharing, or the session ended.
    case closed(reason: String)
}

/// Draws one shared screen and turns touches on it into pointer input.
///
/// Pixels arrive as damaged rectangles, so the picture is kept in one bitmap context and only the
/// rectangles that changed are redrawn. A phone screen is a fraction of a Retina desktop, so the
/// image is drawn to fit and touches are mapped back through the same transform.
@MainActor
final class RemoteScreenViewModel: ObservableObject {
    @Published private(set) var state: RemoteScreenState = .opening
    @Published private(set) var image: CGImage?
    @Published private(set) var control: ScreenControlHolder = .nobody
    /// Set when the device may point; typing needs its own grant.
    @Published private(set) var canControlPointer = false
    @Published private(set) var canControlKeyboard = false

    /// The display being drawn, once the computer's welcome has named one.
    @Published private(set) var displayName: String?

    private let viewer: ScreenViewer
    /// A preview is the computer's thumbnail profile: about one small picture a second.
    private let preview: Bool
    private var surface: UInt32?
    private var canvas: CGContext?
    private var surfaceSize: CGSize = .zero

    /// A `nil` surface means the first display the computer offers, which is all a phone can ask
    /// for before the welcome: a Mac names its displays by their own ids.
    init(
        viewer: ScreenViewer,
        surface: UInt32?,
        ticket: ControllerScreenTicket,
        preview: Bool = false
    ) {
        self.viewer = viewer
        self.surface = surface
        self.preview = preview
        canControlPointer = ticket.canControlPointer
        canControlKeyboard = ticket.canControlKeyboard
    }

    /// The size of the computer's screen, in its own pixels.
    var size: CGSize { surfaceSize }

    /// Lets a test check the fit-and-centre arithmetic without a live session's pixels.
    func setSurfaceSizeForTesting(_ size: CGSize) {
        surfaceSize = size
    }

    /// Applies what arrived in one screen frame and redraws the parts that changed.
    func apply(events: [ScreenEvent]) {
        for event in events {
            switch event {
            case let .welcomed(surfaces, _):
                // Take the first display when nobody named one; the computer knows its own ids.
                let display = surface.flatMap { id in surfaces.first { $0.id == id } }
                    ?? surfaces.first
                guard let display else { continue }
                surface = display.id
                displayName = display.name
            case let .updated(surface, preview, damaged, reset):
                guard surface == self.surface, preview == self.preview else { continue }
                redraw(damaged: damaged, reset: reset)
            case let .control(holder):
                control = holder
            case let .closed(reason):
                state = .closed(reason: reason)
            case .motionRegion, .panes:
                continue
            }
        }
    }

    /// Where a touch at `point` in a view of `viewSize` lands on the computer's screen.
    func surfacePoint(from point: CGPoint, in viewSize: CGSize) -> (x: UInt32, y: UInt32)? {
        guard surfaceSize.width > 0, surfaceSize.height > 0,
              viewSize.width > 0, viewSize.height > 0 else {
            return nil
        }
        // The picture is drawn to fit, centred, without distorting it.
        let scale = min(viewSize.width / surfaceSize.width, viewSize.height / surfaceSize.height)
        let drawn = CGSize(width: surfaceSize.width * scale, height: surfaceSize.height * scale)
        let origin = CGPoint(
            x: (viewSize.width - drawn.width) / 2,
            y: (viewSize.height - drawn.height) / 2
        )
        let onPicture = CGPoint(x: point.x - origin.x, y: point.y - origin.y)
        guard onPicture.x >= 0, onPicture.y >= 0,
              onPicture.x < drawn.width, onPicture.y < drawn.height else {
            return nil
        }
        return (
            UInt32((onPicture.x / scale).rounded(.down)),
            UInt32((onPicture.y / scale).rounded(.down))
        )
    }

    /// Sends a tap as a press and release, when this device may point at all.
    func tap(at point: CGPoint, in viewSize: CGSize) {
        guard !preview, canControlPointer, control == .you,
              let surface,
              let target = surfacePoint(from: point, in: viewSize) else {
            return
        }
        viewer.sendPointerMove(surface: surface, x: target.x, y: target.y)
        viewer.sendPointerButton(
            surface: surface, x: target.x, y: target.y, button: .primary, pressed: true
        )
        viewer.sendPointerButton(
            surface: surface, x: target.x, y: target.y, button: .primary, pressed: false
        )
    }

    /// Redraws the damaged rectangles, or the whole picture when the computer reset it.
    private func redraw(damaged: [ScreenRect], reset: Bool) {
        guard let surface,
              let whole = viewer.surfaceSize(surface: surface, preview: preview) else { return }
        if canvas == nil || surfaceSize != CGSize(width: Int(whole.width), height: Int(whole.height)) {
            surfaceSize = CGSize(width: Int(whole.width), height: Int(whole.height))
            canvas = Self.makeCanvas(width: Int(whole.width), height: Int(whole.height))
        }
        guard let canvas else { return }
        let rects = reset ? [whole] : damaged
        for rect in rects {
            guard let pixels = try? viewer.copyPixels(
                surface: surface, preview: preview, rect: rect
            ) else {
                continue
            }
            draw(pixels, into: canvas, surfaceHeight: Int(whole.height))
        }
        image = canvas.makeImage()
        if case .opening = state, image != nil {
            state = .watching
        }
    }

    private func draw(_ pixels: ScreenPixels, into canvas: CGContext, surfaceHeight: Int) {
        let width = Int(pixels.rect.width)
        let height = Int(pixels.rect.height)
        guard width > 0, height > 0, pixels.bgra.count == width * height * 4 else { return }
        guard let provider = CGDataProvider(data: Data(pixels.bgra) as CFData),
              let tile = CGImage(
                  width: width,
                  height: height,
                  bitsPerComponent: 8,
                  bitsPerPixel: 32,
                  bytesPerRow: width * 4,
                  space: CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue)
                      .union(.byteOrder32Little),
                  provider: provider,
                  decode: nil,
                  shouldInterpolate: false,
                  intent: .defaultIntent
              )
        else {
            return
        }
        // Core Graphics counts rows from the bottom; screens count from the top.
        let flipped = CGRect(
            x: CGFloat(pixels.rect.x),
            y: CGFloat(surfaceHeight - Int(pixels.rect.y) - height),
            width: CGFloat(width),
            height: CGFloat(height)
        )
        canvas.draw(tile, in: flipped)
    }

    private static func makeCanvas(width: Int, height: Int) -> CGContext? {
        guard width > 0, height > 0 else { return nil }
        return CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                | CGBitmapInfo.byteOrder32Little.rawValue
        )
    }
}
