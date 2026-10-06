import SwiftUI

/// Multiplex's in-app keyboard. It never asks iOS to resize the screen for an input view.
struct FlowKeyboard: View {
    let onText: (String, TerminalInputModifiers) -> Void
    let onKey: (TerminalInputKey, TerminalInputModifiers) -> Void
    let onHide: () -> Void
    @State private var shift = false
    @State private var control = false
    @State private var option = false
    @State private var symbols = false
    @State private var typed = ""

    private var modifiers: TerminalInputModifiers {
        TerminalInputModifiers(shift: shift, control: control, alt: option)
    }

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 8) {
                Text(typed.isEmpty ? "Type here · keys send immediately" : typed)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundStyle(typed.isEmpty ? Flow.muted : Flow.text)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .accessibilityLabel(typed.isEmpty ? "Input preview" : "Typed: \(typed)")
                key("⌄", label: "Hide keyboard", action: onHide)
            }
            row(symbols ? Array("1234567890").map(String.init) : Array("qwertyuiop").map(String.init))
            row(symbols ? ["@", "#", "$", "%", "&", "*", "-", "+", "(", ")"] : Array("asdfghjkl").map(String.init))
            HStack(spacing: 4) {
                key("⇧", label: "Shift", selected: shift) { shift.toggle() }
                ForEach(symbols ? ["/", "~", "|", "=", "_", ":", ";"] : Array("zxcvbnm").map(String.init), id: \.self) { character in
                    characterKey(character)
                }
                key("⌫", label: "Backspace") {
                    if !typed.isEmpty { typed.removeLast() }
                    emit(.backspace)
                }
            }
            HStack(spacing: 4) {
                key(symbols ? "ABC" : "123", label: "Letters and symbols") { symbols.toggle() }
                key("ctrl", selected: control) { control.toggle() }
                key("alt", selected: option) { option.toggle() }
                key("space") { type(" ") }
                    .frame(maxWidth: .infinity)
                characterKey(".")
                key("↵", label: "Return") { emit(.enter); typed = "" }
            }
            HStack(spacing: 4) {
                key("esc", label: "Escape") { emit(.escape) }
                key("tab", label: "Tab") { emit(.tab) }
                key("←", label: "Left arrow") { emit(.left) }
                key("↑", label: "Up arrow") { emit(.up) }
                key("↓", label: "Down arrow") { emit(.down) }
                key("→", label: "Right arrow") { emit(.right) }
            }
        }
        .padding(8)
        .background(Flow.surface)
        .overlay(alignment: .top) { Rectangle().fill(Flow.border).frame(height: 1) }
        .onDisappear { typed = "" }
        .accessibilityIdentifier("multiplex-custom-keyboard")
    }

    private func row(_ characters: [String]) -> some View {
        HStack(spacing: 4) {
            ForEach(characters, id: \.self) { characterKey($0) }
        }
    }

    private func characterKey(_ character: String) -> some View {
        let value = shift ? character.uppercased() : character
        return key(value) { type(value) }
    }

    private func key(
        _ title: String,
        label: String? = nil,
        selected: Bool = false,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 15, weight: .medium, design: .monospaced))
                .foregroundStyle(selected ? Flow.accent : Flow.text)
                .frame(maxWidth: .infinity, minHeight: 36)
                .background(selected ? Flow.selection : Flow.raised)
                .clipShape(RoundedRectangle(cornerRadius: Flow.radiusSmall))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label ?? title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func type(_ text: String) {
        onText(text, modifiers)
        typed = control || option ? "" : String((typed + text).suffix(256))
        resetModifiers()
    }

    private func emit(_ key: TerminalInputKey) {
        onKey(key, modifiers)
        resetModifiers()
    }

    private func resetModifiers() {
        shift = false
        control = false
        option = false
    }
}
