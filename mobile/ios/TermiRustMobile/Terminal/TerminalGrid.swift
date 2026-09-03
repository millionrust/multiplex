import CoreGraphics

struct TerminalGrid: Equatable {
    let columns: Int
    let rows: Int
}

func estimateTerminalGrid(size: CGSize, fontSize: CGFloat) -> TerminalGrid {
    let font = max(fontSize, 1)
    let charWidth = max(font * 0.62, 1)
    let lineHeight = max(font * 1.35, 1)

    return TerminalGrid(
        columns: max(20, Int(floor(size.width / charWidth))),
        rows: max(6, Int(floor(size.height / lineHeight)))
    )
}
