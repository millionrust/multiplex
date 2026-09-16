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

/// How a touch on the picture reaches the computer's pointer.
enum RemotePointerMode: String, CaseIterable, Equatable, Sendable {
    /// The pointer goes where the finger lands. Direct, but a finger hides what it touches.
    case direct
    /// The finger drags the pointer from where it was, as a trackpad does, so small targets
    /// stay visible while they are aimed at.
    case trackpad

    var title: String {
        switch self {
        case .direct: return "Touch"
        case .trackpad: return "Trackpad"
        }
    }
}

/// A key the phone can send that a text field cannot type.
struct RemoteScreenKey: Identifiable, Equatable, Sendable {
    let id: String
    let label: String
    /// USB HID usage on the keyboard page; layouts stay the computer's business.
    let usage: UInt16
    var modifiers: UInt8 = 0

    static let escape = RemoteScreenKey(id: "escape", label: "esc", usage: 0x29)
    static let tab = RemoteScreenKey(id: "tab", label: "tab", usage: 0x2B)
    static let control = RemoteScreenKey(id: "control", label: "ctrl", usage: 0xE0)
    static let left = RemoteScreenKey(id: "left", label: "←", usage: 0x50)
    static let up = RemoteScreenKey(id: "up", label: "↑", usage: 0x52)
    static let down = RemoteScreenKey(id: "down", label: "↓", usage: 0x51)
    static let right = RemoteScreenKey(id: "right", label: "→", usage: 0x4F)
    static let pipe = RemoteScreenKey(id: "pipe", label: "|", usage: 0x31, modifiers: 1)
    static let minus = RemoteScreenKey(id: "minus", label: "-", usage: 0x2D)

    /// The row above the keyboard, in the order the design shows them.
    static let accessory: [RemoteScreenKey] = [
        .escape, .tab, .control, .left, .up, .down, .right, .pipe, .minus,
    ]
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
    /// Every display this computer shares, so the viewer can offer a choice.
    @Published private(set) var displays: [ScreenSurface] = []
    /// 1 fits the whole picture; above that the picture is magnified and can be panned.
    @Published private(set) var zoom: CGFloat = 1
    /// How far the magnified picture is dragged from the middle, in view points.
    @Published private(set) var pan: CGSize = .zero
    @Published var pointerMode: RemotePointerMode = .direct
    /// Where the pointer is in trackpad mode, in the computer's own pixels.
    @Published private(set) var pointer: CGPoint = .zero

    /// How many pictures this session has drawn, and when the last one arrived.
    @Published private(set) var picturesDrawn = 0
    @Published private(set) var lastPictureAt: Date?

    /// As far in as a finger may zoom. Past this a phone is magnifying its own blur.
    static let maximumZoom: CGFloat = 6
    /// How long without a picture before the interface says the connection is struggling. A
    /// still screen sends nothing, so this is deliberately longer than a pause in the work.
    static let weakAfter: TimeInterval = 4

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

    /// Lets a test check the fit, zoom and pan arithmetic without a live session's pixels.
    func setSurfaceSizeForTesting(_ size: CGSize) {
        surfaceSize = size
        pointer = CGPoint(x: size.width / 2, y: size.height / 2)
    }

    /// Applies what arrived in one screen frame and redraws the parts that changed.
    func apply(events: [ScreenEvent]) {
        for event in events {
            switch event {
            case let .welcomed(surfaces, _):
                displays = surfaces
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

    /// How many view points one of the computer's pixels takes, at the current zoom.
    func scale(in viewSize: CGSize) -> CGFloat {
        guard surfaceSize.width > 0, surfaceSize.height > 0,
              viewSize.width > 0, viewSize.height > 0 else {
            return 0
        }
        let fitted = min(viewSize.width / surfaceSize.width, viewSize.height / surfaceSize.height)
        return fitted * zoom
    }

    /// The top-left of the drawn picture in the view, once it is fitted, zoomed and dragged.
    func pictureOrigin(in viewSize: CGSize) -> CGPoint {
        let scale = scale(in: viewSize)
        guard scale > 0 else { return .zero }
        let drawn = CGSize(width: surfaceSize.width * scale, height: surfaceSize.height * scale)
        return CGPoint(
            x: (viewSize.width - drawn.width) / 2 + pan.width,
            y: (viewSize.height - drawn.height) / 2 + pan.height
        )
    }

    /// Where a touch at `point` in a view of `viewSize` lands on the computer's screen.
    func surfacePoint(from point: CGPoint, in viewSize: CGSize) -> (x: UInt32, y: UInt32)? {
        let scale = scale(in: viewSize)
        guard scale > 0 else { return nil }
        let origin = pictureOrigin(in: viewSize)
        let onPicture = CGPoint(x: (point.x - origin.x) / scale, y: (point.y - origin.y) / scale)
        guard onPicture.x >= 0, onPicture.y >= 0,
              onPicture.x < surfaceSize.width, onPicture.y < surfaceSize.height else {
            return nil
        }
        return (UInt32(onPicture.x.rounded(.down)), UInt32(onPicture.y.rounded(.down)))
    }

    /// Zooms to `factor`, keeping the middle of the view on the same part of the picture.
    func setZoom(_ factor: CGFloat, in viewSize: CGSize) {
        let clamped = min(max(factor, 1), Self.maximumZoom)
        guard clamped != zoom else { return }
        let before = scale(in: viewSize)
        zoom = clamped
        let after = scale(in: viewSize)
        if before > 0, after > 0 {
            // The point under the middle of the view stays under it.
            pan = CGSize(
                width: pan.width * after / before,
                height: pan.height * after / before
            )
        }
        clampPan(in: viewSize)
    }

    /// Drags the magnified picture. At fit there is nothing to drag.
    func panBy(_ translation: CGSize, in viewSize: CGSize) {
        guard zoom > 1 else { return }
        pan = CGSize(width: pan.width + translation.width, height: pan.height + translation.height)
        clampPan(in: viewSize)
    }

    /// Back to the whole picture.
    func fit() {
        zoom = 1
        pan = .zero
    }

    /// What the zoom chip reads.
    var zoomLabel: String {
        zoom <= 1.01 ? "Fit" : "\(Int((zoom * 100).rounded()))%"
    }

    /// The part of the computer's screen the view is showing, in its own pixels, for the minimap.
    func visibleRect(in viewSize: CGSize) -> CGRect {
        let scale = scale(in: viewSize)
        guard scale > 0 else { return .zero }
        let origin = pictureOrigin(in: viewSize)
        let visible = CGRect(
            x: -origin.x / scale,
            y: -origin.y / scale,
            width: viewSize.width / scale,
            height: viewSize.height / scale
        )
        return visible.intersection(CGRect(origin: .zero, size: surfaceSize))
    }

    /// Keeps the picture from being dragged away from the view entirely.
    private func clampPan(in viewSize: CGSize) {
        let scale = scale(in: viewSize)
        guard scale > 0 else { return }
        let drawn = CGSize(width: surfaceSize.width * scale, height: surfaceSize.height * scale)
        // Once the picture is wider than the view, its edges may not come inside it.
        let slackX = max((drawn.width - viewSize.width) / 2, 0)
        let slackY = max((drawn.height - viewSize.height) / 2, 0)
        pan = CGSize(
            width: min(max(pan.width, -slackX), slackX),
            height: min(max(pan.height, -slackY), slackY)
        )
    }

    /// Whether this device may send anything at all right now.
    var isDriving: Bool { !preview && canControlPointer && control == .you }

    /// Asks the computer for the writer lease. It answers with who holds control.
    func requestControl() {
        guard !preview, canControlPointer || canControlKeyboard else { return }
        viewer.requestControl()
    }

    func releaseControl() {
        guard !preview else { return }
        viewer.releaseControl()
    }

    /// Sends a tap as a press and release, when this device may point at all.
    ///
    /// In trackpad mode the finger has already moved the pointer, so a tap clicks where the
    /// pointer is rather than where the finger landed.
    func tap(at point: CGPoint, in viewSize: CGSize) {
        guard isDriving, let surface else { return }
        let target: (x: UInt32, y: UInt32)
        switch pointerMode {
        case .direct:
            guard let landed = surfacePoint(from: point, in: viewSize) else { return }
            target = landed
            pointer = CGPoint(x: CGFloat(landed.x), y: CGFloat(landed.y))
        case .trackpad:
            target = (UInt32(pointer.x.rounded(.down)), UInt32(pointer.y.rounded(.down)))
        }
        viewer.sendPointerMove(surface: surface, x: target.x, y: target.y)
        viewer.sendPointerButton(
            surface: surface, x: target.x, y: target.y, button: .primary, pressed: true
        )
        viewer.sendPointerButton(
            surface: surface, x: target.x, y: target.y, button: .primary, pressed: false
        )
    }

    /// Moves the pointer the way a trackpad does: by how far the finger went, not where it is.
    func movePointer(by translation: CGSize, in viewSize: CGSize) {
        guard isDriving, pointerMode == .trackpad, let surface else { return }
        let scale = scale(in: viewSize)
        guard scale > 0 else { return }
        let moved = CGPoint(
            x: pointer.x + translation.width / scale,
            y: pointer.y + translation.height / scale
        )
        pointer = CGPoint(
            x: min(max(moved.x, 0), max(surfaceSize.width - 1, 0)),
            y: min(max(moved.y, 0), max(surfaceSize.height - 1, 0))
        )
        viewer.sendPointerMove(
            surface: surface,
            x: UInt32(pointer.x.rounded(.down)),
            y: UInt32(pointer.y.rounded(.down))
        )
    }

    /// Scrolls the computer under the pointer, in its own pixels.
    func scroll(by translation: CGSize, at point: CGPoint, in viewSize: CGSize) {
        guard isDriving, let surface else { return }
        let scale = scale(in: viewSize)
        guard scale > 0 else { return }
        let target = pointerMode == .trackpad
            ? (x: UInt32(pointer.x.rounded(.down)), y: UInt32(pointer.y.rounded(.down)))
            : surfacePoint(from: point, in: viewSize)
        guard let target else { return }
        viewer.sendScroll(
            surface: surface,
            x: target.x,
            y: target.y,
            dx: Int32((translation.width / scale).rounded()),
            dy: Int32((translation.height / scale).rounded())
        )
    }

    /// Types text the computer will insert as typed characters.
    func send(text: String) {
        guard !preview, canControlKeyboard, control == .you, let surface, !text.isEmpty else {
            return
        }
        viewer.sendText(surface: surface, text: text)
    }

    /// Presses and releases one key a text field cannot type.
    func send(key: RemoteScreenKey) {
        guard !preview, canControlKeyboard, control == .you else { return }
        try? viewer.sendKey(usage: key.usage, modifiers: key.modifiers, pressed: true)
        try? viewer.sendKey(usage: key.usage, modifiers: key.modifiers, pressed: false)
    }

    /// Watches a different display of the same computer.
    func watch(display: ScreenSurface) {
        guard display.id != surface else { return }
        if let surface { viewer.unsubscribe(surface: surface) }
        surface = display.id
        displayName = display.name
        canvas = nil
        surfaceSize = .zero
        image = nil
        state = .opening
        fit()
        viewer.subscribe(surface: display.id, preview: preview)
    }

    /// Redraws the damaged rectangles, or the whole picture when the computer reset it.
    private func redraw(damaged: [ScreenRect], reset: Bool) {
        guard let surface,
              let whole = viewer.surfaceSize(surface: surface, preview: preview) else { return }
        if canvas == nil || surfaceSize != CGSize(width: Int(whole.width), height: Int(whole.height)) {
            surfaceSize = CGSize(width: Int(whole.width), height: Int(whole.height))
            canvas = Self.makeCanvas(width: Int(whole.width), height: Int(whole.height))
            // A trackpad pointer starts in the middle, where a person can find it.
            pointer = CGPoint(x: surfaceSize.width / 2, y: surfaceSize.height / 2)
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
        picturesDrawn += 1
        lastPictureAt = Date()
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
