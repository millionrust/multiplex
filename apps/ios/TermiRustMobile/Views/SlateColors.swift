import SwiftUI
import UIKit

extension Color {
    /// A Slate token that follows the system appearance and the Increase Contrast setting.
    static func slate(_ token: @escaping @Sendable (SlateTheme) -> SlateRGBA) -> Color {
        Color(UIColor { trait in
            let theme: SlateTheme
            if trait.accessibilityContrast == .high {
                theme = .highContrast
            } else if trait.userInterfaceStyle == .dark {
                theme = .dark
            } else {
                theme = .light
            }
            let value = token(theme)
            return UIColor(red: value.red, green: value.green, blue: value.blue, alpha: value.alpha)
        })
    }

    /// One of the sixteen named terminal colors, SGR 0 through 15.
    static func slateANSI(_ index: Int) -> Color? {
        let tokens: [@Sendable (SlateTheme) -> SlateRGBA] = [
            SlateTokens.colorTerminalAnsiBlack, SlateTokens.colorTerminalAnsiRed,
            SlateTokens.colorTerminalAnsiGreen, SlateTokens.colorTerminalAnsiYellow,
            SlateTokens.colorTerminalAnsiBlue, SlateTokens.colorTerminalAnsiMagenta,
            SlateTokens.colorTerminalAnsiCyan, SlateTokens.colorTerminalAnsiWhite,
            SlateTokens.colorTerminalAnsiBrightBlack, SlateTokens.colorTerminalAnsiBrightRed,
            SlateTokens.colorTerminalAnsiBrightGreen, SlateTokens.colorTerminalAnsiBrightYellow,
            SlateTokens.colorTerminalAnsiBrightBlue, SlateTokens.colorTerminalAnsiBrightMagenta,
            SlateTokens.colorTerminalAnsiBrightCyan, SlateTokens.colorTerminalAnsiBrightWhite,
        ]
        return tokens.indices.contains(index) ? slate(tokens[index]) : nil
    }

    static let mobileBackground = slate(SlateTokens.colorBgCanvas)
    static let mobilePanelBackground = slate(SlateTokens.colorBgElevated)
    static let panelBorder = slate(SlateTokens.colorBorderDefault)
    static let terminalBackground = slate(SlateTokens.colorBgTerminal)
    static let terminalForeground = slate(SlateTokens.colorTerminalFg)
    static let terminalMuted = slate(SlateTokens.colorTextMuted)
    static let terminalCursor = slate(SlateTokens.colorTerminalCursor)
    static let slateAccent = slate(SlateTokens.colorActionPrimary)
    static let slateDone = slate(SlateTokens.colorStatusDone)
    static let slateAttention = slate(SlateTokens.colorStatusAttention)
    static let slateError = slate(SlateTokens.colorStatusError)
}
