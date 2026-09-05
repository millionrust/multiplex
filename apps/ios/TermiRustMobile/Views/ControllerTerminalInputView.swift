import SwiftUI
import UIKit

struct ControllerTerminalInputView: UIViewRepresentable {
    let enabled: Bool
    @Binding var isFocused: Bool
    let applicationCursor: Bool
    let onBytes: (Data) -> Void
    let onPaste: (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(
            onBytes: onBytes,
            onPaste: onPaste,
            onFocusChange: { isFocused = $0 }
        )
    }

    func makeUIView(context: Context) -> TerminalInputTextView {
        let view = TerminalInputTextView()
        view.delegate = context.coordinator
        view.commandHandler = context.coordinator
        view.backgroundColor = .clear
        view.textColor = .clear
        view.tintColor = .clear
        view.autocorrectionType = .no
        view.autocapitalizationType = .none
        view.spellCheckingType = .no
        view.smartDashesType = .no
        view.smartQuotesType = .no
        view.smartInsertDeleteType = .no
        view.keyboardType = .asciiCapable
        view.returnKeyType = .default
        view.isScrollEnabled = false
        view.accessibilityLabel = "Terminal keyboard input"
        view.inputAccessoryView = context.coordinator.makeAccessory(for: view)
        return view
    }

    func updateUIView(_ view: TerminalInputTextView, context: Context) {
        context.coordinator.onBytes = onBytes
        context.coordinator.onPaste = onPaste
        context.coordinator.onFocusChange = { isFocused = $0 }
        context.coordinator.applicationCursor = applicationCursor
        view.isEditable = enabled
        view.isUserInteractionEnabled = enabled
        if enabled, isFocused, !view.isFirstResponder {
            DispatchQueue.main.async { view.becomeFirstResponder() }
        } else if (!enabled || !isFocused), view.isFirstResponder {
            view.resignFirstResponder()
        }
    }

    @MainActor
    final class Coordinator: NSObject, UITextViewDelegate, TerminalInputCommandHandling {
        var onBytes: (Data) -> Void
        var onPaste: (String) -> Void
        var onFocusChange: (Bool) -> Void
        var applicationCursor = false
        private var controlLatched = false
        private var optionLatched = false
        private var ime = TerminalIMEState()
        private weak var controlButton: UIButton?
        private weak var optionButton: UIButton?

        init(
            onBytes: @escaping (Data) -> Void,
            onPaste: @escaping (String) -> Void,
            onFocusChange: @escaping (Bool) -> Void
        ) {
            self.onBytes = onBytes
            self.onPaste = onPaste
            self.onFocusChange = onFocusChange
        }

        func textViewDidBeginEditing(_ textView: UITextView) {
            onFocusChange(true)
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            onFocusChange(false)
        }

        func textViewDidChange(_ textView: UITextView) {
            if textView.markedTextRange != nil {
                ime.update(textView.text)
                return
            }
            guard !textView.text.isEmpty else {
                ime.cancel()
                return
            }
            _ = ime.commit(textView.text)
            sendText(textView.text)
            textView.text = ""
        }

        func terminalInput(_ input: TerminalInputCommand) {
            switch input {
            case .key(let key, let text, let modifiers):
                sendKey(key, text: text, explicitModifiers: modifiers)
            case .paste(let text):
                onPaste(text)
            case .toggleControl:
                controlLatched.toggle()
                updateModifierButtons()
            case .toggleOption:
                optionLatched.toggle()
                updateModifierButtons()
            }
        }

        func sendText(_ text: String) {
            let bytes = TerminalInteraction.encodeCommittedText(text, modifiers: consumeModifiers())
            if !bytes.isEmpty { onBytes(bytes) }
        }

        private func sendKey(
            _ key: TerminalInputKey,
            text: String? = nil,
            explicitModifiers: TerminalInputModifiers? = nil
        ) {
            let text = text ?? (key == .space ? " " : nil)
            if let bytes = TerminalInteraction.encode(
                key,
                text: text,
                modifiers: explicitModifiers ?? consumeModifiers(),
                applicationCursor: applicationCursor
            ) {
                onBytes(bytes)
            }
        }

        private func consumeModifiers() -> TerminalInputModifiers {
            let modifiers = TerminalInputModifiers(
                control: controlLatched,
                alt: optionLatched
            )
            controlLatched = false
            optionLatched = false
            updateModifierButtons()
            return modifiers
        }

        func makeAccessory(for textView: TerminalInputTextView) -> UIView {
            let accessory = UIInputView(frame: .zero, inputViewStyle: .keyboard)
            accessory.translatesAutoresizingMaskIntoConstraints = false
            accessory.heightAnchor.constraint(equalToConstant: 52).isActive = true
            let scroll = UIScrollView()
            scroll.showsHorizontalScrollIndicator = false
            scroll.translatesAutoresizingMaskIntoConstraints = false
            accessory.addSubview(scroll)
            NSLayoutConstraint.activate([
                scroll.leadingAnchor.constraint(equalTo: accessory.leadingAnchor),
                scroll.trailingAnchor.constraint(equalTo: accessory.trailingAnchor),
                scroll.topAnchor.constraint(equalTo: accessory.topAnchor),
                scroll.bottomAnchor.constraint(equalTo: accessory.bottomAnchor),
            ])
            let row = UIStackView()
            row.axis = .horizontal
            row.spacing = 8
            row.alignment = .center
            row.translatesAutoresizingMaskIntoConstraints = false
            scroll.addSubview(row)
            NSLayoutConstraint.activate([
                row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 8),
                row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -8),
                row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
                row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
                row.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
            ])

            row.addArrangedSubview(button("Esc", command: .key(.escape, nil, nil), textView: textView))
            let control = button("Ctrl", command: .toggleControl, textView: textView)
            let option = button("Alt", command: .toggleOption, textView: textView)
            controlButton = control
            optionButton = option
            row.addArrangedSubview(control)
            row.addArrangedSubview(option)
            row.addArrangedSubview(button("Tab", command: .key(.tab, nil, nil), textView: textView))
            row.addArrangedSubview(button("←", command: .key(.left, nil, nil), textView: textView, label: "Left arrow"))
            row.addArrangedSubview(button("↑", command: .key(.up, nil, nil), textView: textView, label: "Up arrow"))
            row.addArrangedSubview(button("↓", command: .key(.down, nil, nil), textView: textView, label: "Down arrow"))
            row.addArrangedSubview(button("→", command: .key(.right, nil, nil), textView: textView, label: "Right arrow"))
            return accessory
        }

        private func button(
            _ title: String,
            command: TerminalInputCommand,
            textView: TerminalInputTextView,
            label: String? = nil
        ) -> UIButton {
            var configuration = UIButton.Configuration.tinted()
            configuration.title = title
            configuration.cornerStyle = .small
            let button = UIButton(configuration: configuration)
            button.accessibilityLabel = label ?? title
            button.widthAnchor.constraint(greaterThanOrEqualToConstant: 44).isActive = true
            button.heightAnchor.constraint(equalToConstant: 44).isActive = true
            button.addAction(UIAction { [weak textView] _ in
                textView?.commandHandler?.terminalInput(command)
                textView?.becomeFirstResponder()
            }, for: .touchUpInside)
            return button
        }

        private func updateModifierButtons() {
            controlButton?.isSelected = controlLatched
            optionButton?.isSelected = optionLatched
            controlButton?.configuration?.baseBackgroundColor = controlLatched ? .systemGreen : nil
            optionButton?.configuration?.baseBackgroundColor = optionLatched ? .systemGreen : nil
        }

    }
}

@MainActor
protocol TerminalInputCommandHandling: AnyObject {
    func terminalInput(_ input: TerminalInputCommand)
}

enum TerminalInputCommand {
    case key(TerminalInputKey, String?, TerminalInputModifiers?)
    case paste(String)
    case toggleControl
    case toggleOption
}

@MainActor
final class TerminalInputTextView: UITextView {
    weak var commandHandler: TerminalInputCommandHandling?

    override func deleteBackward() {
        if text.isEmpty, markedTextRange == nil {
            commandHandler?.terminalInput(.key(.backspace, nil, nil))
        } else {
            super.deleteBackward()
        }
    }

    override func paste(_ sender: Any?) {
        guard let value = UIPasteboard.general.string, !value.isEmpty else { return }
        commandHandler?.terminalInput(.paste(value))
    }

    override var keyCommands: [UIKeyCommand]? {
        var commands = [
            key(UIKeyCommand.inputEscape, key: .escape, title: "Escape"),
            key("\t", key: .tab, title: "Tab"),
            key("\t", key: .tab, title: "Back Tab", flags: .shift, modifiers: .init(shift: true)),
            key(UIKeyCommand.inputUpArrow, key: .up, title: "Up"),
            key(UIKeyCommand.inputDownArrow, key: .down, title: "Down"),
            key(UIKeyCommand.inputLeftArrow, key: .left, title: "Left"),
            key(UIKeyCommand.inputRightArrow, key: .right, title: "Right"),
        ]
        for character in "abcdefghijklmnopqrstuvwxyz@[\\]^_/ " {
            let text = String(character)
            let terminalKey: TerminalInputKey = character == " " ? .space : .text
            commands.append(key(
                text,
                key: terminalKey,
                title: "Control \(text)",
                flags: .control,
                text: text,
                modifiers: .init(control: true)
            ))
        }
        for character in "abcdefghijklmnopqrstuvwxyz" {
            let text = String(character)
            commands.append(key(
                text,
                key: .text,
                title: "Alt \(text)",
                flags: .alternate,
                text: text,
                modifiers: .init(alt: true)
            ))
        }
        return commands
    }

    @objc private func runKeyCommand(_ command: UIKeyCommand) {
        guard let values = command.propertyList as? [String: Any],
              let raw = values["key"] as? String,
              let key = TerminalInputKey(rawValue: raw) else { return }
        let text = (values["text"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        let modifiers = (values["has_modifiers"] as? Bool ?? false)
            ? TerminalInputModifiers(
                shift: values["shift"] as? Bool ?? false,
                control: values["control"] as? Bool ?? false,
                alt: values["alt"] as? Bool ?? false
            )
            : nil
        commandHandler?.terminalInput(.key(key, text, modifiers))
    }

    private func key(
        _ input: String,
        key: TerminalInputKey,
        title: String,
        flags: UIKeyModifierFlags = [],
        text: String? = nil,
        modifiers: TerminalInputModifiers = .init()
    ) -> UIKeyCommand {
        let command = UIKeyCommand(
            title: title,
            action: #selector(runKeyCommand(_:)),
            input: input,
            modifierFlags: flags,
            propertyList: [
                "key": key.rawValue,
                "text": text ?? "",
                "has_modifiers": !flags.isEmpty,
                "shift": modifiers.shift,
                "control": modifiers.control,
                "alt": modifiers.alt,
            ]
        )
        command.wantsPriorityOverSystemBehavior = true
        return command
    }
}
