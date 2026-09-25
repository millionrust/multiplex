import SwiftUI

/// The design of multiplex-mobile-flow.html, as SwiftUI.
///
/// The app draws itself rather than wearing the platform's: both phones show the same product, so
/// the components here are the prototype's own — its palette, its radii, its type — and not
/// `List`, `Section`, `Picker` or the navigation bar. Values are taken from the prototype's
/// stylesheet and named after the classes there.
enum Flow {
    static let canvas = Color(hex: 0x0F1114)
    static let surface = Color(hex: 0x17191D)
    static let raised = Color(hex: 0x1B1E23)
    static let hover = Color(hex: 0x22262C)
    static let border = Color(hex: 0x2A2E35)
    static let borderSubtle = Color(hex: 0x22262C)
    static let text = Color(hex: 0xE6E8EB)
    static let text2 = Color(hex: 0xB4B9C2)
    static let muted = Color(hex: 0x8C929C)
    static let accent = Color(hex: 0x74A7F2)
    static let accentInk = Color(hex: 0x0E1622)
    static let selection = Color(hex: 0x2B3B52)
    static let good = Color(hex: 0x6FCF97)
    static let warn = Color(hex: 0xE7C07B)
    static let danger = Color(hex: 0xE4736B)
    static let off = Color(hex: 0x4A5361)

    /// `--r` and `--r-sm`.
    static let radius: CGFloat = 14
    static let radiusSmall: CGFloat = 9
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}

/// `.nav`: what this screen is, the way back, and the one action it offers.
struct FlowNav<Action: View>: View {
    let title: String
    var subtitle: String?
    var back: String?
    var onBack: (() -> Void)?
    @ViewBuilder var action: () -> Action

    var body: some View {
        HStack(spacing: 10) {
            if let back, let onBack {
                Button(action: onBack) {
                    HStack(spacing: 3) {
                        Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold))
                        Text(back).font(.system(size: 14))
                    }
                    .foregroundStyle(Flow.accent)
                }
                .buttonStyle(.plain)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(Flow.text)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Flow.muted)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            action()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Flow.canvas)
    }
}

extension FlowNav where Action == EmptyView {
    init(title: String, subtitle: String? = nil, back: String? = nil, onBack: (() -> Void)? = nil) {
        self.init(title: title, subtitle: subtitle, back: back, onBack: onBack) { EmptyView() }
    }
}

/// `.act`: the one word a nav bar offers.
struct FlowAction: View {
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title).font(.system(size: 13.5)).foregroundStyle(Flow.accent)
        }
        .buttonStyle(.plain)
    }
}

/// `.group-label`: the quiet heading over a card.
struct FlowGroupLabel: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 11.5))
            .tracking(0.7)
            .foregroundStyle(Flow.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 4)
            .padding(.top, 14)
            .padding(.bottom, 7)
    }
}

/// `.card`: one group of rows.
struct FlowCard<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(spacing: 0) { content() }
            .background(Flow.surface)
            .clipShape(RoundedRectangle(cornerRadius: Flow.radius))
            .overlay(
                RoundedRectangle(cornerRadius: Flow.radius)
                    .strokeBorder(Flow.borderSubtle, lineWidth: 1)
            )
    }
}

/// `.row`: a glyph, a name, a line under it, and whatever the row ends with.
struct FlowRow<Trailing: View>: View {
    let name: String
    var meta: String?
    /// `.dot` before the meta, when the row has a state to show.
    var dot: Color?
    var glyph: String = "desktopcomputer"
    var glyphOn = false
    var divider = true
    var onTap: (() -> Void)?
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 10)
                    .fill(glyphOn ? Flow.selection : Flow.raised)
                    .frame(width: 36, height: 36)
                    .overlay(
                        Image(systemName: glyph)
                            .font(.system(size: 16))
                            .foregroundStyle(glyphOn ? Flow.accent : Flow.text2)
                    )
                VStack(alignment: .leading, spacing: 1) {
                    Text(name)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(Flow.text)
                        .lineLimit(1)
                    if let meta {
                        HStack(spacing: 6) {
                            if let dot {
                                Circle().fill(dot).frame(width: 7, height: 7)
                            }
                            Text(meta)
                                .font(.system(size: 12))
                                .foregroundStyle(Flow.muted)
                                .lineLimit(1)
                        }
                    }
                }
                Spacer(minLength: 8)
                trailing()
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 13)
            .contentShape(Rectangle())
            .onTapGesture { onTap?() }
            if divider {
                Rectangle().fill(Flow.borderSubtle).frame(height: 1)
            }
        }
    }
}

extension FlowRow where Trailing == FlowChevron {
    init(
        name: String,
        meta: String? = nil,
        dot: Color? = nil,
        glyph: String = "desktopcomputer",
        glyphOn: Bool = false,
        divider: Bool = true,
        onTap: (() -> Void)? = nil
    ) {
        self.init(
            name: name,
            meta: meta,
            dot: dot,
            glyph: glyph,
            glyphOn: glyphOn,
            divider: divider,
            onTap: onTap
        ) { FlowChevron() }
    }
}

/// `.chev`: what a row that goes somewhere ends with.
struct FlowChevron: View {
    var body: some View {
        Image(systemName: "chevron.right")
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(Flow.muted)
    }
}

/// `.badge`: a small count or state at the end of a row.
struct FlowBadge: View {
    let text: String
    var tone: Tone = .plain

    enum Tone { case plain, accent, good, warn }

    var body: some View {
        Text(text)
            .font(.system(size: 10.5))
            .foregroundStyle(foreground)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(background, in: Capsule())
            .overlay(
                Capsule().strokeBorder(tone == .plain ? Flow.border : .clear, lineWidth: 1)
            )
    }

    private var foreground: Color {
        switch tone {
        case .plain: Flow.muted
        case .accent: Flow.accent
        case .good: Flow.good
        case .warn: Flow.warn
        }
    }

    private var background: Color {
        switch tone {
        case .plain: Flow.raised
        case .accent: Flow.selection
        case .good: Color(hex: 0x17301F)
        case .warn: Color(hex: 0x2B2417)
        }
    }
}

/// `.seg`: one control deciding what the page under it is.
struct FlowSegmented<Value: Hashable>: View {
    let options: [(value: Value, title: String)]
    @Binding var selection: Value

    var body: some View {
        HStack(spacing: 3) {
            ForEach(options, id: \.value) { option in
                let on = option.value == selection
                Text(option.title)
                    .font(.system(size: 13, weight: on ? .semibold : .regular))
                    .foregroundStyle(on ? Flow.accent : Flow.muted)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 7)
                    .background(on ? Flow.selection : .clear, in: RoundedRectangle(cornerRadius: 8))
                    .contentShape(Rectangle())
                    .onTapGesture { selection = option.value }
            }
        }
        .padding(3)
        .background(Flow.surface)
        .clipShape(RoundedRectangle(cornerRadius: 11))
        .overlay(
            RoundedRectangle(cornerRadius: 11).strokeBorder(Flow.border, lineWidth: 1)
        )
    }
}

/// `.btn`, `.btn.primary`, `.btn.wide`.
struct FlowButton: View {
    let title: String
    var systemImage: String?
    var primary = false
    var wide = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if let systemImage {
                    Image(systemName: systemImage).font(.system(size: 13, weight: .semibold))
                }
                Text(title).font(.system(size: 13.5, weight: primary ? .semibold : .regular))
            }
            .foregroundStyle(primary ? Flow.accentInk : Flow.text)
            .frame(maxWidth: wide ? .infinity : nil)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(primary ? Flow.accent : Flow.raised)
            .clipShape(RoundedRectangle(cornerRadius: Flow.radiusSmall))
            .overlay(
                RoundedRectangle(cornerRadius: Flow.radiusSmall)
                    .strokeBorder(primary ? Flow.accent : Flow.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

/// `.body`: the scrolling part of a screen.
struct FlowBody<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        ScrollView {
            VStack(spacing: 0) { content() }
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 22)
        }
        .background(Flow.canvas)
    }
}
