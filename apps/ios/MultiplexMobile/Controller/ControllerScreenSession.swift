import Foundation

/// Watching a paired computer's screen over the Controller channel.
///
/// The Controller connection carries the bytes; this owns what they mean. A screen session starts
/// with an `open_screen` command, which answers with a one-time ticket, and then rides screen
/// frames: watching traffic under `observeScreens`, pointer input under `controlPointer`, typing
/// under `controlKeyboard`. The computer checks that claim again on its side, so a frame labelled
/// as watching can never carry a keystroke.
enum ControllerScreenCommand {
    static let open = "open_screen"
    static let close = "close_screen"

    /// The body of an `open_screen` command, for the envelope the connection already builds.
    static func openBody() -> [String: Any] {
        ["kind": open]
    }

    static func closeBody() -> [String: Any] {
        ["kind": close]
    }
}

/// What the computer answered when a screen session was opened.
struct ControllerScreenTicket: Equatable, Sendable {
    let commandId: UUID
    let ticket: Data
    let canControlPointer: Bool
    let canControlKeyboard: Bool

    var canControl: Bool { canControlPointer || canControlKeyboard }
}

enum ControllerScreenError: Error, Equatable {
    /// The response was not the one this command asked for, or carried unexpected fields.
    case malformedResponse
    /// A ticket is exactly 32 bytes.
    case invalidTicket
    /// The device may watch but was never granted what this input needs.
    case notGranted
}

enum ControllerScreenResponse {
    static let ticketBytes = 32

    /// Parses a `screen_opened` response, rejecting anything with unexpected fields.
    static func ticket(from data: Data) throws -> ControllerScreenTicket {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              Set(object.keys) == [
                  "kind", "command_id", "ticket", "can_control_pointer", "can_control_keyboard",
              ],
              object["kind"] as? String == "screen_opened",
              let identifier = object["command_id"] as? String,
              let commandId = UUID(uuidString: identifier),
              let bytes = object["ticket"] as? [Any],
              let pointer = object["can_control_pointer"] as? Bool,
              let keyboard = object["can_control_keyboard"] as? Bool
        else {
            throw ControllerScreenError.malformedResponse
        }
        guard bytes.count == ticketBytes else {
            throw ControllerScreenError.invalidTicket
        }
        var ticket = Data(capacity: ticketBytes)
        for value in bytes {
            guard let number = value as? Int, (0...255).contains(number) else {
                throw ControllerScreenError.invalidTicket
            }
            ticket.append(UInt8(number))
        }
        return ControllerScreenTicket(
            commandId: commandId,
            ticket: ticket,
            canControlPointer: pointer,
            canControlKeyboard: keyboard
        )
    }
}

/// Moves bytes between the screen viewer and the Controller connection.
///
/// The viewer says which capability each outgoing chunk needs; this refuses to send one the
/// device was never granted, so the phone does not ask the computer to close the session on its
/// behalf.
struct ControllerScreenPump {
    let ticket: ControllerScreenTicket

    /// The Controller capability a screen frame must claim for this chunk.
    static func capability(for outgoing: ScreenCapability) -> ControllerCapability {
        switch outgoing {
        case .observe: .observeScreens
        case .pointer: .controlPointer
        case .keyboard: .controlKeyboard
        }
    }

    /// Whether this device may send a chunk that claims `capability`.
    func allows(_ capability: ScreenCapability) -> Bool {
        switch capability {
        case .observe: true
        case .pointer: ticket.canControlPointer
        case .keyboard: ticket.canControlKeyboard
        }
    }

    /// Everything the viewer has queued, ready to seal, in order.
    func drain(_ viewer: ScreenViewer) throws -> [(ControllerCapability, Data)] {
        var frames: [(ControllerCapability, Data)] = []
        while let outgoing = viewer.pollOutgoing() {
            guard allows(outgoing.capability) else {
                throw ControllerScreenError.notGranted
            }
            frames.append((Self.capability(for: outgoing.capability), outgoing.bytes))
        }
        return frames
    }
}
