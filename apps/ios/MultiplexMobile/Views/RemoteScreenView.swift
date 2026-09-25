import SwiftUI

/// One computer's screen: drawn to fit, zoomed and panned by hand, and driven once the computer
/// hands this device control.
///
/// Watching is the default and needs no permission beyond the pairing. Pointing and typing appear
/// only once the computer gives control, so the interface never offers an action the other end
/// would refuse.
struct RemoteScreenView: View {
    @ObservedObject var model: RemoteScreenViewModel
    let onRequestControl: () -> Void
    let onReleaseControl: () -> Void
    /// Set while the session is being opened again; the last picture stays on screen.
    var reconnecting = false
    /// What this phone is connected over, for the connection sheet.
    var routeName: String?
    /// Whether the picture shares the screen with something below it, and so keeps its own shape
    /// instead of filling what is left.
    var fitsPicture = false

    @State private var zoomAnchor: CGFloat = 1
    @State private var showingKeyboard = false
    @State private var showingDisplays = false
    @State private var showingConnection = false
    @State private var typed = ""
    @State private var now = Date()
    @FocusState private var typing: Bool

    /// Drives the "no picture for a while" test; nothing arrives to trigger it by itself.
    private let clock = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        VStack(spacing: 0) {
            if weak {
                weakBanner
            }
            picture
                .frame(maxHeight: fitsPicture ? nil : .infinity)
            if showingKeyboard, model.canControlKeyboard {
                keyboard
            }
            dock
        }
        .background(Color.mobileBackground)
        .onReceive(clock) { now = $0 }
        .confirmationDialog("Displays", isPresented: $showingDisplays, titleVisibility: .visible) {
            ForEach(model.displays, id: \.id) { display in
                Button(ControllerPresentation.isolated(display.name)) {
                    model.watch(display: display)
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(isPresented: $showingConnection) {
            RemoteScreenConnectionSheet(
                model: model,
                routeName: routeName,
                reconnecting: reconnecting,
                now: now
            )
            .presentationDetents([.medium])
        }
    }

    /// Whether the picture has gone still for longer than a pause in the work would explain.
    private var weak: Bool {
        guard case .watching = model.state, !reconnecting else { return false }
        guard let last = model.lastPictureAt else { return false }
        return now.timeIntervalSince(last) > RemoteScreenViewModel.weakAfter
    }

    private var weakBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(Color.slateAttention)
            VStack(alignment: .leading, spacing: 1) {
                Text("Weak connection")
                    .font(.footnote.weight(.semibold))
                Text("No new pictures for a moment. Nothing has been lost.")
                    .font(.caption2)
                    .foregroundStyle(Color.terminalMuted)
            }
            Spacer(minLength: 4)
            Button("Details") { showingConnection = true }
                .font(.caption)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(Color.slateAttention.opacity(0.12))
    }

    @ViewBuilder
    private var picture: some View {
        switch model.state {
        case .opening:
            centered {
                ProgressView()
                    .tint(Color.terminalMuted)
            }
        case .watching:
            GeometryReader { geometry in
                ZStack(alignment: .topTrailing) {
                    stage(in: geometry.size)
                    if model.zoom > 1.01 {
                        RemoteScreenMinimap(
                            visible: model.visibleRect(in: geometry.size),
                            surface: model.size
                        )
                        .padding(12)
                    }
                    zoomChip
                        .padding(12)
                        .opacity(model.zoom > 1.01 ? 0 : 1)
                }
            }
        case let .closed(reason):
            centered {
                Text(reason)
                    .font(.footnote)
                    .foregroundStyle(Color.terminalMuted)
                    .multilineTextAlignment(.center)
            }
        }
    }

    private func stage(in size: CGSize) -> some View {
        ZStack {
            if let image = model.image {
                Image(decorative: image, scale: 1, orientation: .up)
                    .resizable()
                    .interpolation(.none)
                    .aspectRatio(contentMode: .fit)
                    .scaleEffect(model.zoom)
                    .offset(x: model.pan.width, y: model.pan.height)
                    .clipped()
            }
            if model.pointerMode == .trackpad, model.isDriving {
                RemoteScreenPointer(
                    at: model.pointer,
                    scale: model.scale(in: size),
                    origin: model.pictureOrigin(in: size)
                )
            }
            if reconnecting {
                // Over the last picture, not instead of it: a frozen picture of the right
                // computer says more than an empty one.
                VStack(spacing: 8) {
                    ProgressView()
                        .tint(.white)
                    Text("Reconnecting…")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.white)
                }
                .padding(20)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
            }
        }
        .frame(width: size.width, height: size.height)
        .contentShape(Rectangle())
        .gesture(drag(in: size))
        .simultaneousGesture(magnification(in: size))
        .onTapGesture { location in model.tap(at: location, in: size) }
        .onTapGesture(count: 2) { _ in model.fit() }
    }

    /// One finger pans the magnified picture, or drives the pointer in trackpad mode.
    private func drag(in size: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { value in
                let step = CGSize(
                    width: value.translation.width - lastDrag.width,
                    height: value.translation.height - lastDrag.height
                )
                lastDrag = value.translation
                if model.pointerMode == .trackpad, model.isDriving {
                    model.movePointer(by: step, in: size)
                } else {
                    model.panBy(step, in: size)
                }
            }
            .onEnded { _ in lastDrag = .zero }
    }

    private func magnification(in size: CGSize) -> some Gesture {
        MagnifyGesture()
            .onChanged { value in
                model.setZoom(zoomAnchor * value.magnification, in: size)
            }
            .onEnded { _ in zoomAnchor = model.zoom }
    }

    @State private var lastDrag: CGSize = .zero

    private var zoomChip: some View {
        Text(model.zoomLabel)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.ultraThinMaterial, in: Capsule())
            .accessibilityLabel("Zoom \(model.zoomLabel)")
    }

    /// The row of keys a text field cannot type, above a field that types everything else.
    private var keyboard: some View {
        VStack(spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(RemoteScreenKey.accessory) { key in
                        Button(key.label) { model.send(key: key) }
                            .font(.footnote.monospaced())
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(Color.secondary.opacity(0.16), in: RoundedRectangle(cornerRadius: 6))
                            .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 12)
            }
            TextField("Type on this computer", text: $typed)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .focused($typing)
                .onChange(of: typed) { _, text in
                    guard !text.isEmpty else { return }
                    model.send(text: text)
                    typed = ""
                }
                .padding(.horizontal, 12)
        }
        .padding(.vertical, 8)
        .background(Color.secondary.opacity(0.08))
    }

    private var dock: some View {
        HStack(spacing: 14) {
            if model.canControlKeyboard {
                Button {
                    showingKeyboard.toggle()
                    typing = showingKeyboard
                } label: {
                    Label("Keyboard", systemImage: "keyboard")
                        .labelStyle(.iconOnly)
                }
                .disabled(!model.isDriving)
            }
            if model.canControlPointer {
                Button {
                    model.pointerMode = model.pointerMode == .direct ? .trackpad : .direct
                } label: {
                    Label(model.pointerMode.title, systemImage: pointerIcon)
                        .labelStyle(.iconOnly)
                }
                .disabled(!model.isDriving)
                .accessibilityValue(model.pointerMode.title)
            }
            if model.displays.count > 1 {
                Button {
                    showingDisplays = true
                } label: {
                    Label("Displays", systemImage: "rectangle.on.rectangle")
                        .labelStyle(.iconOnly)
                }
            }
            Button {
                showingConnection = true
            } label: {
                Label("Connection", systemImage: "antenna.radiowaves.left.and.right")
                    .labelStyle(.iconOnly)
            }
            Spacer(minLength: 4)
            // A glyph, like the rest of the dock: as words these read as a paragraph of buttons
            // over the desktop they are meant to leave alone.
            if model.canControlPointer || model.canControlKeyboard {
                Button {
                    if model.control == .you {
                        onReleaseControl()
                    } else {
                        onRequestControl()
                    }
                } label: {
                    Label(
                        model.control == .you ? "Stop controlling" : "Take control",
                        systemImage: "cursorarrow.click"
                    )
                    .labelStyle(.iconOnly)
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.control == .anotherDevice)
            } else {
                // Nothing here at all used to be the only sign that this computer never granted
                // pointer or keyboard, which reads as a missing button rather than an answer.
                Text("Control not granted")
                    .font(.footnote)
                    .foregroundStyle(Color.terminalMuted)
                    .lineLimit(1)
                    .accessibilityLabel(
                        "This computer has not granted pointer or keyboard control to this phone."
                    )
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private var pointerIcon: String {
        model.pointerMode == .direct ? "hand.point.up.left" : "rectangle.and.hand.point.up.left"
    }

    private var controlLabel: String {
        switch model.control {
        case .you: "You are controlling this computer"
        case .anotherDevice: "Another device is controlling it"
        case .nobody: "Watching only"
        }
    }

    private func centered<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack {
            Spacer()
            content()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// What this phone actually knows about the connection carrying the screen.
///
/// Stage A rides the Controller channel, which reports no round-trip time or loss, so this shows
/// what the phone can see for itself rather than inventing numbers. Bandwidth, loss and the
/// steps that reduce detail arrive with the motion path.
private struct RemoteScreenConnectionSheet: View {
    @ObservedObject var model: RemoteScreenViewModel
    let routeName: String?
    let reconnecting: Bool
    let now: Date

    var body: some View {
        NavigationStack {
            List {
                Section {
                    LabeledContent("Route", value: routeName ?? "Private network")
                    LabeledContent("Pictures", value: "\(model.picturesDrawn)")
                    LabeledContent("Last picture", value: sinceLastPicture)
                    if let name = model.displayName {
                        LabeledContent("Display", value: ControllerPresentation.isolated(name))
                    }
                    LabeledContent("Size", value: sizeText)
                }
                Section {
                    Text(reconnecting
                        ? "The session dropped and is being opened again. The picture on screen is the last one that arrived."
                        : "A still screen sends nothing, so a pause between pictures is usually the computer being quiet rather than the network.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                } header: {
                    Text("Keeping up")
                } footer: {
                    Text("Reducing detail to keep a busy screen moving arrives with the motion path; Stage A always sends exact pixels.")
                }
            }
            .navigationTitle("Connection")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private var sinceLastPicture: String {
        guard let last = model.lastPictureAt else { return "None yet" }
        let seconds = Int(now.timeIntervalSince(last).rounded())
        return seconds <= 0 ? "Just now" : "\(seconds)s ago"
    }

    private var sizeText: String {
        let size = model.size
        guard size.width > 0 else { return "Unknown" }
        return "\(Int(size.width)) × \(Int(size.height))"
    }
}

/// Where the view is looking, when the picture is bigger than the view.
private struct RemoteScreenMinimap: View {
    let visible: CGRect
    let surface: CGSize

    private static let width: CGFloat = 76

    var body: some View {
        let height = surface.width > 0 ? Self.width * surface.height / surface.width : 0
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 3)
                .fill(Color.black.opacity(0.35))
            if surface.width > 0, surface.height > 0, height > 0 {
                Rectangle()
                    .strokeBorder(Color.white.opacity(0.9), lineWidth: 1)
                    .frame(
                        width: Self.width * visible.width / surface.width,
                        height: height * visible.height / surface.height
                    )
                    .offset(
                        x: Self.width * visible.minX / surface.width,
                        y: height * visible.minY / surface.height
                    )
            }
        }
        .frame(width: Self.width, height: max(height, 1))
        .accessibilityLabel("Minimap")
    }
}

/// The trackpad pointer, drawn where the computer's pointer now is.
private struct RemoteScreenPointer: View {
    let at: CGPoint
    let scale: CGFloat
    let origin: CGPoint

    var body: some View {
        Circle()
            .strokeBorder(Color.white, lineWidth: 1.5)
            .background(Circle().fill(Color.black.opacity(0.35)))
            .frame(width: 18, height: 18)
            .position(x: origin.x + at.x * scale, y: origin.y + at.y * scale)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}
