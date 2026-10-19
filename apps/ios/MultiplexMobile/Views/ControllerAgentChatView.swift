import SwiftUI

/// Chat or Terminal, offered over a terminal while a coding agent runs in it.
struct ControllerAgentChatSwitch: View {
    @ObservedObject var chat: ControllerAgentChatViewModel
    @Binding var showsChat: Bool

    var body: some View {
        Group {
            if let current = chat.chat {
                HStack(spacing: 10) {
                    FlowSegmented(
                        options: [
                            (true, String(localized: "Chat")),
                            (false, String(localized: "Terminal")),
                        ],
                        selection: $showsChat
                    )
                    ControllerAgentStatePill(provider: current.provider, state: current.state)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(Flow.canvas)
            }
        }
        .onChange(of: chat.available) { _, available in
            if !available { showsChat = false }
        }
    }
}

struct ControllerAgentStatePill: View {
    let provider: ControllerAgentProvider
    let state: ControllerAgentChatState

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(state == .working ? Flow.warn : Flow.good)
                .frame(width: 6, height: 6)
            Text(state == .working ? String(localized: "Working") : String(localized: "Ready"))
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(Flow.text2)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Flow.raised, in: Capsule())
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(provider.displayName), \(state == .working ? "working" : "ready")")
    }
}

/// The agent's conversation: what was asked, what it answered, and the tools it ran, folded
/// away. Replies are typed into the terminal as a person would type them.
struct ControllerAgentChatView: View {
    @ObservedObject var chat: ControllerAgentChatViewModel
    @ObservedObject var terminal: ControllerTerminalViewModel
    let onShowTerminal: () -> Void
    @State private var atBottom = true
    @State private var newerBelow = false
    @FocusState private var composing: Bool

    private var providerName: String { chat.chat?.provider.displayName ?? "Agent" }

    var body: some View {
        VStack(spacing: 0) {
            if let message = chat.message {
                Label(message, systemImage: "wifi.exclamationmark")
                    .font(.caption)
                    .foregroundStyle(Flow.warn)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .background(Flow.surface)
            }
            thread
            composer
        }
        .background(Flow.canvas)
    }

    private var thread: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    header
                    ForEach(chat.items) { item in
                        ControllerAgentChatItemView(
                            item: item,
                            providerName: providerName,
                            expanded: Binding(
                                get: { chat.expandedTools.contains(item.id) },
                                set: { open in
                                    if open {
                                        chat.expandedTools.insert(item.id)
                                    } else {
                                        chat.expandedTools.remove(item.id)
                                    }
                                }
                            )
                        )
                        .id(item.id)
                    }
                    if chat.chat?.state == .working {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text(String(localized: "\(providerName) is working…"))
                                .font(.system(size: 12.5))
                                .foregroundStyle(Flow.muted)
                        }
                    }
                    Color.clear
                        .frame(height: 1)
                        .id("bottom")
                        .onAppear {
                            atBottom = true
                            newerBelow = false
                        }
                        .onDisappear { atBottom = false }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 14)
            }
            .scrollDismissesKeyboard(.interactively)
            .onAppear { proxy.scrollTo("bottom", anchor: .bottom) }
            .onChange(of: chat.items.last?.id) { _, _ in
                if atBottom {
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("bottom", anchor: .bottom) }
                } else {
                    newerBelow = true
                }
            }
            .overlay(alignment: .bottom) {
                if newerBelow {
                    Button {
                        withAnimation { proxy.scrollTo("bottom", anchor: .bottom) }
                        newerBelow = false
                    } label: {
                        Label(String(localized: "Latest"), systemImage: "arrow.down")
                            .font(.system(size: 12.5, weight: .semibold))
                            .foregroundStyle(Flow.accentInk)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                            .background(Flow.accent, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .padding(.bottom, 10)
                }
            }
        }
    }

    @ViewBuilder
    private var header: some View {
        if let title = chat.chat?.title {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Flow.text2)
                .frame(maxWidth: .infinity)
                .multilineTextAlignment(.center)
        }
        if chat.hasEarlier {
            Button {
                chat.loadEarlier()
            } label: {
                HStack(spacing: 6) {
                    if chat.loadingEarlier { ProgressView().controlSize(.small) }
                    Text(String(localized: "Earlier messages"))
                }
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(Flow.accent)
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(.plain)
            .disabled(chat.loadingEarlier)
        } else if chat.chat?.historyTruncated == true {
            Text(String(localized: "Older messages are too far back in the log to show here."))
                .font(.system(size: 11.5))
                .foregroundStyle(Flow.muted)
                .frame(maxWidth: .infinity)
                .multilineTextAlignment(.center)
        }
        if chat.items.isEmpty {
            Text(String(localized: "Nothing said yet. Send \(providerName) a message, or use the terminal."))
                .font(.system(size: 13))
                .foregroundStyle(Flow.muted)
                .frame(maxWidth: .infinity)
                .multilineTextAlignment(.center)
                .padding(.top, 40)
        }
    }

    @ViewBuilder
    private var composer: some View {
        VStack(spacing: 6) {
            if terminal.privacyCovered {
                EmptyView()
            } else if terminal.canSendInput {
                HStack(alignment: .bottom, spacing: 8) {
                    TextField(
                        String(localized: "Message \(providerName)…"),
                        text: $chat.draft,
                        axis: .vertical
                    )
                    .lineLimit(1...6)
                    .font(.system(size: 15))
                    .foregroundStyle(Flow.text)
                    .focused($composing)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .background(Flow.surface, in: RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Flow.border))
                    Button {
                        if terminal.sendChatMessage(chat.draft) {
                            chat.draft = ""
                        }
                    } label: {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 15, weight: .bold))
                            .foregroundStyle(Flow.accentInk)
                            .frame(width: 38, height: 38)
                            .background(Flow.accent, in: RoundedRectangle(cornerRadius: 10))
                    }
                    .buttonStyle(.plain)
                    .disabled(chat.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .opacity(chat.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.45 : 1)
                    .accessibilityLabel(String(localized: "Send"))
                }
                Text(String(localized: "Sent to the terminal as if typed. Answer prompts and approvals in Terminal."))
                    .font(.system(size: 10.5))
                    .foregroundStyle(Flow.muted)
                    .frame(maxWidth: .infinity)
                    .multilineTextAlignment(.center)
            } else {
                HStack(spacing: 10) {
                    Text(terminal.supportsWriterControl
                        ? String(localized: "Take control of the terminal to reply.")
                        : String(localized: "This phone can watch but not type into this terminal."))
                        .font(.system(size: 12.5))
                        .foregroundStyle(Flow.text2)
                    Spacer(minLength: 4)
                    if terminal.supportsWriterControl {
                        FlowButton(title: String(localized: "Take Control"), primary: true) {
                            terminal.requestControl()
                        }
                    } else {
                        FlowButton(title: String(localized: "Terminal"), action: onShowTerminal)
                    }
                }
                if let message = terminal.writerMessage {
                    Text(message)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Flow.warn)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 10)
        .background(Flow.canvas)
        .overlay(alignment: .top) { Rectangle().fill(Flow.border).frame(height: 1) }
    }
}

struct ControllerAgentChatItemView: View {
    let item: ControllerAgentChatItem
    let providerName: String
    @Binding var expanded: Bool

    var body: some View {
        switch item.kind {
        case .user: user
        case .assistant: assistant
        case .tool: tool
        case .notice: notice
        }
    }

    private var user: some View {
        HStack {
            Spacer(minLength: 36)
            VStack(alignment: .trailing, spacing: 4) {
                Text(item.text)
                    .font(.system(size: 15))
                    .foregroundStyle(Flow.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(
                        Flow.raised,
                        in: UnevenRoundedRectangle(
                            topLeadingRadius: 13,
                            bottomLeadingRadius: 13,
                            bottomTrailingRadius: 4,
                            topTrailingRadius: 13
                        )
                    )
                shortened
            }
        }
    }

    private var assistant: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                Text(providerName == "Codex" ? "C" : "✳")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Flow.accent)
                    .frame(width: 19, height: 19)
                    .background(Flow.selection, in: RoundedRectangle(cornerRadius: 5))
                Text(providerName)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Flow.muted)
            }
            ForEach(Array(ControllerChatSegment.split(item.text).enumerated()), id: \.offset) { _, segment in
                switch segment {
                case .prose(let text):
                    Text(Self.markdown(text))
                        .font(.system(size: 15))
                        .foregroundStyle(Flow.text)
                        .tint(Flow.accent)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                case .code(_, let text):
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(text)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(Flow.text)
                            .textSelection(.enabled)
                            .fixedSize()
                            .padding(11)
                    }
                    .background(Color(hex: 0x0B0D10), in: RoundedRectangle(cornerRadius: 8))
                }
            }
            shortened
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var tool: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.15)) { expanded.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Flow.muted)
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                    Text(item.title ?? String(localized: "Tool"))
                        .font(.system(size: 12.5, design: .monospaced))
                        .foregroundStyle(Flow.text2)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 6)
                    toolState
                }
                .padding(.horizontal, 11)
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expanded {
                Rectangle().fill(Flow.borderSubtle).frame(height: 1)
                if let title = item.title {
                    Text(title)
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundStyle(Flow.text2)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 11)
                        .padding(.top, 9)
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(item.text.isEmpty ? String(localized: "No output yet.") : item.text)
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundStyle(item.text.isEmpty ? Flow.muted : Flow.text)
                        .textSelection(.enabled)
                        .fixedSize()
                        .padding(11)
                }
                shortened.padding(.horizontal, 11).padding(.bottom, 8)
            }
        }
        .background(Flow.surface, in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Flow.border))
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private var toolState: some View {
        switch item.toolState {
        case .running:
            ProgressView().controlSize(.mini)
        case .failed:
            Image(systemName: "xmark.circle")
                .font(.system(size: 13))
                .foregroundStyle(Flow.danger)
                .accessibilityLabel(String(localized: "Failed"))
        default:
            Image(systemName: "checkmark")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Flow.good)
                .accessibilityLabel(String(localized: "Done"))
        }
    }

    private var notice: some View {
        Text(item.text)
            .font(.system(size: 11.5))
            .foregroundStyle(Flow.muted)
            .frame(maxWidth: .infinity)
            .multilineTextAlignment(.center)
    }

    @ViewBuilder
    private var shortened: some View {
        if item.truncated {
            Text(String(localized: "Shortened. The terminal has all of it."))
                .font(.system(size: 10.5))
                .foregroundStyle(Flow.muted)
        }
    }

    /// Inline Markdown — bold, italics, code, links — as the agents write it; text that does
    /// not parse stays as it was.
    static func markdown(_ text: String) -> AttributedString {
        (try? AttributedString(
            markdown: text,
            options: AttributedString.MarkdownParsingOptions(
                interpretedSyntax: .inlineOnlyPreservingWhitespace,
                failurePolicy: .returnPartiallyParsedIfPossible
            )
        )) ?? AttributedString(text)
    }
}
