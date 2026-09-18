import CryptoKit
import Darwin
import Foundation

/// Replays the recorded screen session through the generated Swift bindings, so the phone
/// boundary is proven against the bytes a host really sends rather than against a Swift mock.
@main
enum ScreenBindingConformanceRunner {
    static func main() {
        do {
            let fixturePath = try required(CommandLine.arguments.dropFirst().first)
            let session = try GoldenSession(path: fixturePath)
            try replay(session)
            try refuseNonsense(session)
            print("RS3-SWIFT OK")
        } catch {
            fputs("RS3-SWIFT FAIL: \(error)\n", stderr)
            exit(1)
        }
    }

    /// What a phone does: feed every screen frame, then draw what changed.
    private static func replay(_ session: GoldenSession) throws {
        let viewer = ScreenViewer(cacheBytes: 16 << 20)
        try viewer.connect(ticket: session.ticket)

        // The hello goes out before anything arrives, and it is not input.
        let hello = try required(viewer.pollOutgoing())
        try require(hello.capability == .observe, "the hello is watching traffic")
        try require(!hello.bytes.isEmpty, "the hello carries bytes")

        var updates: UInt32 = 0
        var damaged: [ScreenRect] = []
        for frame in session.hostFrames {
            for event in try viewer.receive(bytes: frame) {
                if case let .updated(surface, preview, rects, _) = event {
                    try require(surface == session.surfaceId, "updates name the shared surface")
                    try require(!preview, "this session subscribed to the full view")
                    updates += 1
                    damaged.append(contentsOf: rects)
                }
            }
        }
        try require(updates == session.expectedUpdates, "\(updates) updates, expected \(session.expectedUpdates)")
        try require(!damaged.isEmpty, "something was reported as damaged")

        let whole = ScreenRect(x: 0, y: 0, width: session.width, height: session.height)
        let size = try required(viewer.surfaceSize(surface: session.surfaceId, preview: false))
        try require(size == whole, "the surface is the size the host announced")

        let pixels = try viewer.copyPixels(surface: session.surfaceId, preview: false, rect: whole)
        try require(pixels.rect == whole, "the copy covers what was asked for")
        var digest = ""
        for byte in SHA256.hash(data: pixels.bgra) {
            digest += String(format: "%02x", byte)
        }
        try require(digest == session.pixelsSha256, "the picture differs from the one Rust replayed")

        // Every damaged rectangle can be drawn on its own, which is how a phone repaints.
        for rect in damaged {
            let part = try viewer.copyPixels(surface: session.surfaceId, preview: false, rect: rect)
            try require(
                part.bgra.count == Int(rect.width) * Int(rect.height) * 4,
                "a damaged rectangle copies its own pixels"
            )
        }
    }

    /// The boundary refuses what a caller cannot mean, before the computer has to.
    private static func refuseNonsense(_ session: GoldenSession) throws {
        let viewer = ScreenViewer(cacheBytes: 1 << 20)
        try requireThrows("a short ticket") { try viewer.connect(ticket: Data(repeating: 7, count: 31)) }
        try requireThrows("undefined modifier bits") {
            try viewer.sendKey(usage: 0x04, modifiers: 0xF0, pressed: true)
        }
        try requireThrows("an empty rectangle") {
            try viewer.setViewport(
                surface: session.surfaceId,
                rect: ScreenRect(x: 0, y: 0, width: 0, height: 8),
                scaleMilli: 1000
            )
        }
        try requireThrows("a session id that is not 16 bytes") {
            try viewer.attachPanes(sessions: [Data(repeating: 1, count: 15)])
        }
        try requireThrows("pixels from a surface nobody subscribed to") {
            _ = try viewer.copyPixels(
                surface: session.surfaceId,
                preview: false,
                rect: ScreenRect(x: 0, y: 0, width: 8, height: 8)
            )
        }
        try requireThrows("bytes that are not a screen session") {
            _ = try viewer.receive(bytes: Data(repeating: 0xFF, count: 64))
        }
    }
}

private struct GoldenSession {
    let ticket: Data
    let hostFrames: [Data]
    let surfaceId: UInt32
    let width: UInt32
    let height: UInt32
    let expectedUpdates: UInt32
    let pixelsSha256: String

    init(path: String) throws {
        let raw = try Data(contentsOf: URL(fileURLWithPath: path))
        let json = try required(try JSONSerialization.jsonObject(with: raw) as? [String: Any])
        let surface = try required(json["surface"] as? [String: Any])
        ticket = try Self.data(try required(json["ticket_hex"] as? String))
        hostFrames = try required(json["host_frames_hex"] as? [String]).map(Self.data)
        surfaceId = UInt32(try required(surface["id"] as? Int))
        width = UInt32(try required(surface["width"] as? Int))
        height = UInt32(try required(surface["height"] as? Int))
        expectedUpdates = UInt32(try required(json["expected_updates"] as? Int))
        pixelsSha256 = try required(json["pixels_sha256"] as? String)
    }

    private static func data(_ hex: String) throws -> Data {
        var bytes = Data(capacity: hex.count / 2)
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = try required(hex.index(index, offsetBy: 2, limitedBy: hex.endIndex))
            bytes.append(try required(UInt8(hex[index..<next], radix: 16)))
            index = next
        }
        return bytes
    }
}

private struct ConformanceFailure: Error, CustomStringConvertible {
    let description: String
}

private func require(_ condition: Bool, _ reason: String) throws {
    if !condition {
        throw ConformanceFailure(description: reason)
    }
}

private func required<T>(_ value: T?) throws -> T {
    guard let value else {
        throw ConformanceFailure(description: "a required fixture value was missing")
    }
    return value
}

private func requireThrows(_ reason: String, _ body: () throws -> Void) throws {
    do {
        try body()
    } catch {
        return
    }
    throw ConformanceFailure(description: "\(reason) was accepted")
}
