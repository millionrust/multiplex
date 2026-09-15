import SwiftUI

struct ControllerReadOnlyTerminalView: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @ObservedObject var viewModel: ControllerTerminalViewModel
    let onClose: () -> Void
    @AppStorage("controllerTerminalFontSize") private var terminalFontSize = 14.0
    @AppStorage("controllerTerminalDesktopWidth") private var usesDesktopWidth = false
    @State private var followsOutput = true
    @State private var keyboardPresented = false
    @State private var displayedTerminalFontSize = 14.0
    @State private var displayedTerminalColumns = 40

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if !usesFocusedLandscapeLayout { statusBar }
                terminalSurface
                if viewModel.canSendInput, usesFocusedLandscapeLayout {
                    focusedLandscapeInputBar
                }
            }
            .background(Color.black)
            .navigationTitle(ControllerPresentation.isolated(viewModel.sessionTitle))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(action: onClose) {
                        Image(systemName: "xmark")
                    }
                    .accessibilityLabel("Detach")
                }
                ToolbarItemGroup(placement: .primaryAction) {
                    if viewModel.canSendInput {
                        Button { keyboardPresented.toggle() } label: {
                            Image(systemName: keyboardPresented
                                ? "keyboard.chevron.compact.down" : "keyboard")
                        }
                        .accessibilityLabel(
                            keyboardPresented ? "Hide Keyboard" : "Show Keyboard"
                        )
                    }
                    terminalMenu
                }
            }
            .toolbar(
                usesFocusedLandscapeLayout ? .hidden : .visible,
                for: .navigationBar
            )
        }
        .tint(.green)
        .onAppear { viewModel.start() }
        .onDisappear { viewModel.detach() }
        .confirmationDialog(
            "Send pasted text to this terminal?",
            isPresented: Binding(
                get: { viewModel.pendingPasteByteCount > 0 },
                set: { if !$0 { viewModel.cancelPaste() } }
            ),
            titleVisibility: .visible
        ) {
            Button("Send Paste") { viewModel.confirmPaste() }
            Button("Cancel", role: .cancel) { viewModel.cancelPaste() }
        } message: {
            Text("The paste contains multiple lines or is large (\(viewModel.pendingPasteByteCount) bytes). Review the destination before sending it.")
        }
    }

    private var statusBar: some View {
        VStack(alignment: .leading, spacing: 0) {
            ViewThatFits(in: .horizontal) {
                statusRow(compact: false)
                statusRow(compact: true)
            }
            if let message = viewModel.writerMessage {
                Label(message, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityLabel("Terminal control warning. \(message)")
                    .padding(.horizontal, 10)
                    .padding(.bottom, 7)
            }
            if let message = viewModel.connectionMessage {
                Label(message, systemImage: "wifi.exclamationmark")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityLabel("Terminal connection warning. \(message)")
                    .padding(.horizontal, 10)
                    .padding(.bottom, 7)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground))
        .overlay(alignment: .bottom) { Divider() }
    }

    @ViewBuilder
    private func statusRow(compact: Bool) -> some View {
        HStack(spacing: compact ? 7 : 9) {
            Circle()
                .fill(statusColor)
                .frame(width: 7, height: 7)
                .accessibilityHidden(true)
            Text(ControllerPresentation.isolated(viewModel.hostTitle))
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
                .layoutPriority(1)
            if !compact {
                Text(shortStatusText)
                    .font(.caption)
                    .foregroundStyle(statusColor)
                    .lineLimit(1)
            }
            Spacer(minLength: 2)
            if compact {
                Label(writerLabel, systemImage: writerIcon)
                    .labelStyle(.iconOnly)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(writerColor)
                    .accessibilityLabel("Terminal control status: \(writerLabel)")
            } else {
                Label(writerLabel, systemImage: writerIcon)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(writerColor)
                    .lineLimit(1)
                    .accessibilityLabel("Terminal control status: \(writerLabel)")
            }
            controlAction(compact: compact)
        }
        .padding(.horizontal, 10)
        .frame(height: TerminalAcceptance.minimumTouchTarget)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(
            "\(viewModel.hostTitle), \(statusText), sequence \(viewModel.outputSequence)"
        )
    }

    @ViewBuilder
    private func controlAction(compact: Bool) -> some View {
        if viewModel.writerLease == .held {
            Button(action: viewModel.releaseControl) {
                if compact {
                    Image(systemName: "hand.raised")
                } else {
                    Label("Release", systemImage: "hand.raised")
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .frame(minWidth: 44, minHeight: TerminalAcceptance.minimumTouchTarget)
            .accessibilityLabel("Release Control")
            .accessibilityHint("Returns this terminal to view-only mode")
        } else if viewModel.canRequestControl {
            Button(action: viewModel.requestControl) {
                if compact {
                    Image(systemName: "hand.tap")
                } else {
                    Label("Control", systemImage: "hand.tap")
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .frame(minWidth: 44, minHeight: TerminalAcceptance.minimumTouchTarget)
            .accessibilityLabel("Request Control")
            .accessibilityHint("Requests the single writer lease for this exact session")
        }
        if showsRetry {
            Button(action: viewModel.retry) {
                if compact {
                    Image(systemName: "arrow.clockwise")
                } else {
                    Label("Retry", systemImage: "arrow.clockwise")
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .frame(minWidth: 44, minHeight: TerminalAcceptance.minimumTouchTarget)
            .accessibilityLabel("Retry Connection")
        }
    }

    private var terminalMenu: some View {
        Menu {
            Section("Text Size") {
                Button("Decrease", systemImage: "minus") {
                    terminalFontSize = max(
                        TerminalAcceptance.minimumFontSize,
                        terminalFontSize - 1
                    )
                }
                .disabled(terminalFontSize <= TerminalAcceptance.minimumFontSize)
                Button("Increase", systemImage: "plus") {
                    terminalFontSize = min(
                        TerminalAcceptance.maximumFontSize,
                        terminalFontSize + 1
                    )
                }
                .disabled(terminalFontSize >= TerminalAcceptance.maximumFontSize)
            }
            Section("Terminal Width") {
                Button {
                    usesDesktopWidth = false
                } label: {
                    Label(
                        "Phone Width",
                        systemImage: usesDesktopWidth ? "rectangle" : "checkmark"
                    )
                }
                Button {
                    usesDesktopWidth = true
                } label: {
                    Label(
                        "Desktop Width",
                        systemImage: usesDesktopWidth ? "checkmark" : "rectangle.wide"
                    )
                }
            }
            Button { followsOutput.toggle() } label: {
                Label(
                    followsOutput ? "Stop Following Output" : "Follow Output",
                    systemImage: followsOutput
                        ? "arrow.down.to.line.compact" : "arrow.down"
                )
            }
            if !viewModel.visibleHTTPURLs.isEmpty {
                Menu("Open Link", systemImage: "link") {
                    ForEach(viewModel.visibleHTTPURLs, id: \.absoluteString) { url in
                        Button(url.absoluteString) { openURL(url) }
                    }
                }
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityLabel("Terminal Options")
        .accessibilityValue("Text size \(Int(terminalFontSize)) points")
    }

    private var terminalSurface: some View {
        GeometryReader { geometry in
            Group {
                if viewModel.privacyCovered {
                    privacyCover
                } else {
                    ZStack(alignment: .bottomTrailing) {
                        terminalOutput(availableWidth: geometry.size.width)
                        ControllerTerminalInputView(
                            enabled: viewModel.canSendInput,
                            isFocused: $keyboardPresented,
                            applicationCursor: viewModel.screen.applicationCursor,
                            onBytes: viewModel.sendKeyboardBytes,
                            onPaste: viewModel.requestPaste
                        )
                        .frame(width: 2, height: 2)
                        .opacity(0.01)
                        .accessibilityHidden(!viewModel.canSendInput)
                    }
                }
            }
            .onAppear { updateViewport(for: geometry.size) }
            .onChange(of: geometry.size) { _, size in updateViewport(for: size) }
            .onChange(of: terminalFontSize) { _, _ in updateViewport(for: geometry.size) }
            .onChange(of: usesDesktopWidth) { _, _ in
                updateViewport(for: geometry.size, final: true)
            }
            .onChange(of: dynamicTypeSize) { _, _ in updateViewport(for: geometry.size) }
            .accessibilityElement(children: .contain)
        }
    }

    @ViewBuilder
    private func terminalOutput(availableWidth: CGFloat) -> some View {
        Group {
            if usesDesktopWidth {
                ScrollViewReader { horizontalReader in
                    ScrollView(.horizontal) {
                        verticalTerminalOutput(
                            availableWidth: availableWidth,
                            showsCursorAnchor: true
                        )
                    }
                    .onChange(of: viewModel.renderRevision) { _, _ in
                        guard followsOutput,
                              viewModel.screen.cursorVisible else { return }
                        withAnimation(.none) {
                            horizontalReader.scrollTo(
                                ControllerTerminalCursor.anchorID,
                                anchor: .trailing
                            )
                        }
                    }
                }
            } else {
                verticalTerminalOutput(
                    availableWidth: availableWidth,
                    showsCursorAnchor: false
                )
            }
        }
        .clipped()
        .background(Color.black)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Terminal output for \(viewModel.sessionTitle)")
        .accessibilityValue(accessibleTerminalOutput)
        .accessibilityHint(
            followsOutput
                ? "Following the latest output"
                : "Output following is paused"
        )
        .scrollBounceBehavior(.basedOnSize)
    }

    private func verticalTerminalOutput(
        availableWidth: CGFloat,
        showsCursorAnchor: Bool
    ) -> some View {
        ScrollViewReader { reader in
            ScrollView(.vertical) {
                terminalRows(
                    availableWidth: availableWidth,
                    showsCursorAnchor: showsCursorAnchor
                )
            }
            .onChange(of: viewModel.renderRevision) { _, _ in
                guard followsOutput,
                      let row = ControllerTerminalFollowTarget.row(
                        lines: viewModel.screen.lines,
                        cursorRow: viewModel.screen.cursorRow,
                        scrollbackRows: viewModel.screen.scrollbackRows
                      ) else { return }
                withAnimation(.none) {
                    reader.scrollTo(row, anchor: .bottom)
                }
            }
        }
    }

    @ViewBuilder
    private func terminalRows(
        availableWidth: CGFloat,
        showsCursorAnchor: Bool
    ) -> some View {
        LazyVStack(alignment: .leading, spacing: 0) {
            if viewModel.screen.lines.allSatisfy(\.isEmpty) {
                Text(emptyText)
                    .foregroundStyle(Color.white.opacity(0.55))
                    .padding(16)
            } else {
                ForEach(
                    Array(viewModel.screen.contentCells.enumerated()),
                    id: \.offset
                ) { index, row in
                    ZStack(alignment: .leading) {
                        Text(attributedRow(row, rowIndex: index))
                            .lineLimit(1)
                            .fixedSize(horizontal: true, vertical: false)
                        if showsCursorAnchor,
                           let column = cursorColumn(for: row, rowIndex: index) {
                            Color.clear
                                .frame(width: 1, height: cursorLineHeight)
                                .offset(x: CGFloat(column) * terminalCellWidth)
                                .id(ControllerTerminalCursor.anchorID)
                                .allowsHitTesting(false)
                        }
                    }
                    .frame(minHeight: cursorLineHeight)
                    .id(index)
                }
            }
        }
        .textSelection(.enabled)
        .padding(12)
        .frame(
            width: terminalContentWidth(available: availableWidth),
            alignment: .leading
        )
    }

    private var focusedLandscapeInputBar: some View {
        HStack(spacing: 10) {
            Label("You Control", systemImage: "hand.tap.fill")
                .font(.caption.weight(.bold))
                .foregroundStyle(.green)
            Spacer(minLength: 8)
            Button("Release") { viewModel.releaseControl() }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .frame(minHeight: 44)
            Button { keyboardPresented = false } label: {
                Label("Hide Keyboard", systemImage: "keyboard.chevron.compact.down")
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .frame(minHeight: 44)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 2)
        .background(Color(uiColor: .secondarySystemBackground))
        .overlay(alignment: .top) { Divider() }
    }

    private var usesFocusedLandscapeLayout: Bool {
        ControllerTerminalLayout.usesFocusedLandscape(
            verticalSizeClassIsCompact: verticalSizeClass == .compact,
            keyboardPresented: keyboardPresented
        )
    }

    private var privacyCover: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            Label("Terminal hidden while TermiRust is inactive", systemImage: "eye.slash")
                .font(.headline)
                .multilineTextAlignment(.center)
                .padding(24)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Terminal content hidden for privacy")
    }

    private func updateViewport(for size: CGSize, final: Bool = false) {
        let layout = TerminalAcceptance.layout(
            width: size.width,
            height: size.height,
            requestedFontSize: terminalFontSize,
            textScale: terminalTextScale
        )
        let columns = ControllerTerminalWidth.columns(
            fitting: layout.columns,
            usesDesktopWidth: usesDesktopWidth
        )
        displayedTerminalFontSize = layout.displayedFontSize
        displayedTerminalColumns = columns
        viewModel.updateViewport(columns: columns, rows: layout.rows, final: final)
    }

    private func terminalContentWidth(available: CGFloat) -> CGFloat {
        guard usesDesktopWidth else { return available }
        let gridWidth = CGFloat(displayedTerminalColumns)
            * CGFloat(displayedTerminalFontSize * 0.62)
            + 24
        return max(available, gridWidth)
    }

    private var terminalCellWidth: CGFloat {
        CGFloat(displayedTerminalFontSize * 0.62)
    }

    private var cursorLineHeight: CGFloat {
        CGFloat(displayedTerminalFontSize * 1.35)
    }

    private func cursorColumn(
        for cells: [BoundedTerminalCell],
        rowIndex: Int
    ) -> Int? {
        ControllerTerminalCursor.column(
            rowIndex: rowIndex,
            cells: cells,
            cursorRow: viewModel.screen.cursorRow,
            cursorColumn: viewModel.screen.cursorColumn,
            scrollbackRows: viewModel.screen.scrollbackRows,
            visible: viewModel.screen.cursorVisible
        )
    }

    private func attributedRow(
        _ cells: [BoundedTerminalCell],
        rowIndex: Int
    ) -> AttributedString {
        var row = AttributedString()
        let cursorColumn = cursorColumn(for: cells, rowIndex: rowIndex)
        var displayCells = cells
        if let cursorColumn {
            while displayCells.count <= cursorColumn {
                displayCells.append(.blank())
            }
        }
        for (column, cell) in displayCells.enumerated()
            where cell.width != .continuation {
            var segment = AttributedString(cell.text)
            let colors = resolvedColors(for: cell.style)
            if column == cursorColumn {
                segment.foregroundColor = .black
                segment.backgroundColor = Color.green.opacity(0.9)
            } else {
                segment.foregroundColor = colors.foreground
                segment.backgroundColor = colors.background
            }
            var font = Font.system(
                size: CGFloat(displayedTerminalFontSize),
                design: .monospaced
            )
            if cell.style.bold { font = font.weight(.bold) }
            if cell.style.italic { font = font.italic() }
            segment.font = font
            if cell.style.underline { segment.underlineStyle = .single }
            row.append(segment)
        }
        return row
    }

    private var accessibleTerminalOutput: String {
        let output = TerminalAcceptance.accessibleOutput(lines: viewModel.screen.lines)
        return output.isEmpty ? emptyText : output
    }

    private var terminalTextScale: Double {
        switch dynamicTypeSize {
        case .xSmall, .small, .medium, .large: 1
        case .xLarge: 1.12
        case .xxLarge: 1.24
        case .xxxLarge: 1.35
        case .accessibility1: 1.6
        case .accessibility2: 1.8
        case .accessibility3: 2
        case .accessibility4: 2.2
        case .accessibility5: 2.4
        @unknown default: 1
        }
    }

    private func resolvedColors(
        for style: TerminalCellStyle
    ) -> (foreground: Color, background: Color) {
        let normalForeground = terminalColor(style.foreground, default: Color(white: 0.92))
        let normalBackground = terminalColor(style.background, default: .black)
        let foreground = style.inverse ? normalBackground : normalForeground
        let background = style.inverse ? normalForeground : normalBackground
        return (
            style.dim ? foreground.opacity(0.55) : foreground,
            background
        )
    }

    private func terminalColor(_ color: TerminalCellColor, default fallback: Color) -> Color {
        switch color {
        case .default:
            fallback
        case .indexed(let value):
            ansiColor(Int(value))
        case .rgb(let red, let green, let blue):
            Color(
                red: Double(red) / 255,
                green: Double(green) / 255,
                blue: Double(blue) / 255
            )
        }
    }

    private func ansiColor(_ index: Int) -> Color {
        let base: [(Double, Double, Double)] = [
            (0, 0, 0), (0.80, 0, 0), (0, 0.80, 0), (0.80, 0.80, 0),
            (0, 0, 0.80), (0.80, 0, 0.80), (0, 0.80, 0.80), (0.75, 0.75, 0.75),
            (0.50, 0.50, 0.50), (1, 0, 0), (0, 1, 0), (1, 1, 0),
            (0.35, 0.35, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)
        ]
        if base.indices.contains(index) {
            let value = base[index]
            return Color(red: value.0, green: value.1, blue: value.2)
        }
        if (16...231).contains(index) {
            let cube = index - 16
            let levels = [0, 95, 135, 175, 215, 255]
            return Color(
                red: Double(levels[cube / 36]) / 255,
                green: Double(levels[(cube / 6) % 6]) / 255,
                blue: Double(levels[cube % 6]) / 255
            )
        }
        let gray = Double(8 + max(0, min(23, index - 232)) * 10) / 255
        return Color(red: gray, green: gray, blue: gray)
    }

    private var writerLabel: String {
        guard viewModel.supportsWriterControl else { return "View Only" }
        switch viewModel.writerLease {
        case .none: return viewModel.hasWriterElsewhere ? "Controlled Elsewhere" : "View Only"
        case .requesting: return "Requesting Control"
        case .held: return "You Control"
        case .busy: return "Controlled Elsewhere"
        case .lost: return "Control Lost"
        }
    }

    private var writerIcon: String {
        switch viewModel.writerLease {
        case .held: "hand.tap.fill"
        case .requesting: "clock"
        case .busy: "person.crop.circle.badge.exclamationmark"
        case .lost: "exclamationmark.shield"
        case .none: "eye"
        }
    }

    private var writerColor: Color {
        switch viewModel.writerLease {
        case .held: .green
        case .requesting: .blue
        case .busy, .lost: .orange
        case .none: viewModel.hasWriterElsewhere ? .orange : .blue
        }
    }

    private var showsRetry: Bool {
        switch viewModel.attachState {
        case .offline, .gap, .failed: true
        default: false
        }
    }

    private var statusText: String {
        switch viewModel.attachState {
        case .detached: "Detached"
        case .authenticating: "Authenticating"
        case .snapshot: "Restoring retained screen"
        case .replaying: "Replaying output"
        case .live: viewModel.hasWriterElsewhere ? "Live - writer active elsewhere" : "Live"
        case .gap(let expected, let received):
            "Output gap - expected \(expected), received \(received)"
        case .exited: "Session exited - retained output"
        case .offline: "Offline - showing last screen"
        case .failed: "Terminal stopped at a safety limit"
        }
    }

    private var statusColor: Color {
        switch viewModel.attachState {
        case .live: .green
        case .gap, .failed: .orange
        case .offline, .exited: .secondary
        default: .blue
        }
    }

    private var shortStatusText: String {
        switch viewModel.attachState {
        case .authenticating: "Connecting"
        case .snapshot, .replaying: "Restoring"
        case .gap, .failed: "Needs attention"
        case .exited: "Exited"
        case .offline: "Offline"
        case .detached: "Detached"
        case .live: "Live"
        }
    }

    private var emptyText: String {
        switch viewModel.attachState {
        case .authenticating, .snapshot, .replaying: "Waiting for terminal output..."
        case .live: "The session has not produced visible output."
        case .offline: "No retained screen is available while this Host is offline."
        case .failed: "Terminal output could not be rendered safely."
        default: "No terminal output."
        }
    }
}

enum ControllerTerminalFollowTarget {
    static func row(lines: [String], cursorRow: Int, scrollbackRows: Int) -> Int? {
        guard !lines.isEmpty else { return nil }
        let cursor = min(
            lines.count - 1,
            max(0, scrollbackRows + cursorRow)
        )
        let lastContent = lines.lastIndex {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        } ?? 0
        return max(cursor, lastContent)
    }
}

enum ControllerTerminalCursor {
    static let anchorID = "controller-terminal-cursor"

    static func column(
        rowIndex: Int,
        cells: [BoundedTerminalCell],
        cursorRow: Int,
        cursorColumn: Int,
        scrollbackRows: Int,
        visible: Bool
    ) -> Int? {
        guard visible,
              rowIndex == max(0, scrollbackRows + cursorRow) else {
            return nil
        }
        let column = max(0, cursorColumn)
        if cells.indices.contains(column), cells[column].width == .continuation {
            return max(0, column - 1)
        }
        return column
    }
}

enum ControllerTerminalLayout {
    static func usesFocusedLandscape(
        verticalSizeClassIsCompact: Bool,
        keyboardPresented: Bool
    ) -> Bool {
        verticalSizeClassIsCompact && keyboardPresented
    }
}

enum ControllerTerminalWidth {
    static let desktopColumns = 80

    static func columns(fitting columns: Int, usesDesktopWidth: Bool) -> Int {
        usesDesktopWidth ? max(columns, desktopColumns) : columns
    }
}
