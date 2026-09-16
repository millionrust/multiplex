import SwiftUI

/// One computer's screen, drawn to fit, with a tap that lands where it looks like it lands.
///
/// Watching is the default and needs no permission beyond the pairing. Pointing appears only once
/// the computer hands this device control, so the interface never offers an action that would be
/// refused on the other end.
struct RemoteScreenView: View {
    @ObservedObject var model: RemoteScreenViewModel
    let onRequestControl: () -> Void
    let onReleaseControl: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            picture
            controlBar
        }
        .background(Color.mobileBackground)
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
                ZStack {
                    if let image = model.image {
                        Image(decorative: image, scale: 1, orientation: .up)
                            .resizable()
                            .interpolation(.none)
                            .aspectRatio(contentMode: .fit)
                    }
                }
                .frame(width: geometry.size.width, height: geometry.size.height)
                .contentShape(Rectangle())
                .onTapGesture { location in
                    model.tap(at: location, in: geometry.size)
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

    @ViewBuilder
    private var controlBar: some View {
        if model.canControlPointer || model.canControlKeyboard {
            HStack {
                Text(controlLabel)
                    .font(.footnote)
                    .foregroundStyle(Color.terminalMuted)
                Spacer()
                Button(model.control == .you ? "Stop controlling" : "Take control") {
                    if model.control == .you {
                        onReleaseControl()
                    } else {
                        onRequestControl()
                    }
                }
                .font(.footnote)
                .disabled(model.control == .anotherDevice)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
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
