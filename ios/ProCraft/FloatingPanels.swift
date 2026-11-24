import SwiftUI

// The Layers panel and the brush library torn off into windows of their own, while a plugin lets
// them (the floating_panels hook): each dragged out by a grabber at its top, as the colour panel
// is, then a window over the canvas that stays open while one draws, moved by the strip at its
// top, sized from its lower right corner and closed by its Close (its button then hangs it from
// the bar again). Inside, the panel is the same as where it hangs; the window is kept where it was
// left. Where each goes is the core's to work out (PanelLayout, see PanelLayoutModel.swift): while
// a plugin lays the panels out, they snap to the screen's edges and to each other, fold to a tab
// by a button beside Close, and those docked may make room for the canvas.

/// The strip a floating panel's window adds above the panel: its grabber, and Close at its right.
let floatingTopHeight: CGFloat = 24
/// How far down a floating window's grabber is (at the window's own size): what is held when it
/// is dragged out.
let floatingGrabY: CGFloat = 12
/// The strip a floating panel's window adds under the panel, where its corner sizes it from.
private let floatingBottomHeight: CGFloat = 20
/// How far a panel's grabber is dragged before the panel floats, as the colour panel's.
let panelUndockDrag: CGFloat = 30
/// What the strips over and under a floating panel take from its window's height.
let floatingStrips = floatingTopHeight + floatingBottomHeight

/// The panels that may float, and which is being dragged out; where their windows are is the
/// panel layout's (CanvasModel's `floatingWindow` and the rest, PanelLayoutModel.swift).
@Observable
final class FloatingPanels {
    /// The panels that may float, in the order their windows are drawn.
    static let panels: [FloatingPanel] = [.layers, .brushes]

    /// A panel's grabber is being dragged out: the panel hanging stays, unseen, until let go, its
    /// window following the finger.
    var undocking: FloatingPanel?
    /// Likewise any other piece being dragged out (the Actions panel, a toolbar).
    var undockingPiece: LayoutPiece?
    /// The sizes told the layout of the pieces that aren't sized, so the same size tells nothing.
    @ObservationIgnored var sizesTold: [LayoutPiece: CGSize] = [:]

    /// Whether a plugin lets `panel` float now.
    func allowed(_ panel: FloatingPanel) -> Bool {
        _ = PluginsModel.shared.changes
        return PluginsModel.shared.store.floats(panel: panel)
    }

    /// How big the windows show next to the rest of the interface, as a plugin says (1: as the rest).
    var scale: Float {
        Float(Look.shared.scale(.floating))
    }
}

/// What a panel's window adds to it while it floats: how wide it shows the panel, and what its
/// grabber strip and its corner do (pressed, `onHold`, sizing or not; dragged, `onDrag` by all the
/// drag has gone, in interface points; then `onLetGo`), its Close (`onClose`) and, while the panels
/// fold, its fold button (`onFold`).
struct FloatingFrame {
    let width: CGFloat
    let height: CGFloat
    let onHold: (Bool) -> Void
    let onDrag: (CGSize) -> Void
    let onLetGo: () -> Void
    let onClose: () -> Void
    var onFold: (() -> Void)?
    var menu: WindowMenu?
}

/// A floating window's menu, from the button on its strip: Fold, Hide, Pin to Top (or Unpin) and
/// Reset Position.
struct WindowMenu {
    let pinned: Bool
    let onFold: (() -> Void)?
    let onHide: () -> Void
    let onPin: () -> Void
    let onReset: () -> Void
}

/// `piece`'s window menu, as the layout has it now.
func windowMenu(_ model: CanvasModel, _ piece: LayoutPiece) -> WindowMenu {
    let pinned = model.pinnedPiece(piece)
    return WindowMenu(
        pinned: pinned,
        onFold: model.layoutLook.fold ? { model.foldPiece(piece, true) } : nil,
        onHide: { model.hidePiece(piece) },
        onPin: { model.pinPiece(piece, !pinned) },
        onReset: { model.resetPiece(piece) }
    )
}

/// The window's menu button: three dots on a disc, opening its menu.
struct FloatingMenuButton: View {
    let menu: WindowMenu

    var body: some View {
        Menu {
            if let fold = menu.onFold {
                Button("Fold", action: fold)
            }
            Button("Hide", action: menu.onHide)
            Button(menu.pinned ? "Unpin" : "Pin to Top", action: menu.onPin)
            Button("Reset Position", action: menu.onReset)
        } label: {
            Canvas { context, size in
                let o = CGPoint(x: size.width / 2, y: size.height / 2)
                let r: CGFloat = 8.5
                context.fill(Path(ellipseIn: CGRect(x: o.x - r, y: o.y - r, width: r * 2, height: r * 2)), with: .color(.white.opacity(0.15)))
                for dx: CGFloat in [-3.6, 0, 3.6] {
                    context.fill(Path(ellipseIn: CGRect(x: o.x + dx - 1.1, y: o.y - 1.1, width: 2.2, height: 2.2)), with: .color(.black.opacity(0.35)))
                }
            }
            .frame(width: 40, height: 40)
        }
        .accessibilityLabel("Window")
    }
}

/// A drag that holds `frame`'s piece as it starts (`sizing` or not), followed all told in the
/// interface's points, and lets go.
private struct HeldDrag: ViewModifier {
    let frame: FloatingFrame
    let sizing: Bool
    @State private var held = false

    func body(content: Content) -> some View {
        content.gesture(
            DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { drag in
                    if !held {
                        held = true
                        frame.onHold(sizing)
                    }
                    let k = Interface.shared.scale
                    frame.onDrag(CGSize(width: drag.translation.width / k, height: drag.translation.height / k))
                }
                .onEnded { _ in
                    held = false
                    frame.onLetGo()
                }
        )
    }
}

/// The strip over a floating panel: its grabber in the middle (the whole strip drags the window),
/// Close at its right, and left of it the fold button while the panels fold.
struct FloatingTopStrip: View {
    let frame: FloatingFrame

    var body: some View {
        ZStack(alignment: .top) {
            Color.clear
                .contentShape(Rectangle())
                .modifier(HeldDrag(frame: frame, sizing: false))
            Capsule()
                .fill(Pc.grabber)
                .frame(width: 40, height: 4)
                .padding(.top, 10)
                .allowsHitTesting(false)
            FloatingPanelClose(onClose: frame.onClose)
                .padding(.trailing, 4)
                .offset(y: -8)
                .frame(maxWidth: .infinity, alignment: .trailing)
            if let fold = frame.onFold {
                FloatingFold(onFold: fold)
                    .padding(.trailing, 44)
                    .offset(y: -8)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            if let menu = frame.menu {
                FloatingMenuButton(menu: menu)
                    .padding(.leading, 6)
                    .offset(y: -8)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(height: floatingTopHeight)
        .accessibilityElement(children: .contain)
    }
}

/// The room a floating panel's window leaves under the panel, for its corner (`FloatingCorner`).
struct FloatingBottomStrip: View {
    var body: some View {
        Color.clear.frame(height: floatingBottomHeight)
    }
}

/// Close: a disc with a cross cut out of it, as the floating colour window's.
private struct FloatingPanelClose: View {
    let onClose: () -> Void

    var body: some View {
        Canvas { context, size in
            let o = CGPoint(x: size.width / 2, y: size.height / 2)
            let r: CGFloat = 8.5
            context.fill(Path(ellipseIn: CGRect(x: o.x - r, y: o.y - r, width: r * 2, height: r * 2)), with: .color(.white.opacity(0.15)))
            let d: CGFloat = 3.2
            var cross = Path()
            cross.move(to: CGPoint(x: o.x - d, y: o.y - d))
            cross.addLine(to: CGPoint(x: o.x + d, y: o.y + d))
            cross.move(to: CGPoint(x: o.x + d, y: o.y - d))
            cross.addLine(to: CGPoint(x: o.x - d, y: o.y + d))
            context.stroke(cross, with: .color(.black.opacity(0.3)), lineWidth: 1.3)
        }
        .frame(width: 40, height: 40)
        .contentShape(Rectangle())
        .onTapGesture(perform: onClose)
        .accessibilityLabel("Close")
        .accessibilityAddTraits(.isButton)
    }
}

/// A floating panel's lower right corner, two short lines across it: dragged, it sizes the window.
struct FloatingCorner: View {
    let frame: FloatingFrame

    var body: some View {
        Canvas { context, size in
            // two lines across the corner, the nearer shorter
            let end = CGPoint(x: size.width - 12, y: size.height - 6)
            for reach: CGFloat in [10, 5] {
                var line = Path()
                line.move(to: CGPoint(x: end.x - reach, y: end.y))
                line.addLine(to: CGPoint(x: end.x, y: end.y - reach))
                context.stroke(line, with: .color(Pc.grabber), style: StrokeStyle(lineWidth: 2, lineCap: .round))
            }
        }
        .frame(width: 36, height: floatingBottomHeight)
        .padding(.trailing, 6)
        .contentShape(Rectangle())
        .modifier(HeldDrag(frame: frame, sizing: true))
        .accessibilityLabel("Resize")
    }
}

/// The grabber at the top of a panel hanging from its button, while a plugin lets it float: dragged
/// out past `panelUndockDrag`, the panel goes unseen and its window follows the finger; let go, it
/// floats there.
struct PanelGrabber: View {
    let model: CanvasModel
    let panel: FloatingPanel

    var body: some View {
        ZStack(alignment: .top) {
            Capsule()
                .fill(Pc.grabber)
                .frame(width: 40, height: 4)
                .padding(.top, 4)
                .allowsHitTesting(false)
            // (narrower than the colour panel's: clear of a title beside it)
            Color.clear
                .frame(width: 100, height: 30)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 1, coordinateSpace: .global)
                        .onChanged { drag in
                            guard model.floating.undocking == panel || hypot(drag.translation.width, drag.translation.height) > panelUndockDrag else { return }
                            model.undockPanel(panel, at: Interface.shared.point(drag.location))
                        }
                        .onEnded { _ in
                            if model.undockedPanel(panel) {
                                model.closePanels()
                            }
                        }
                )
        }
        .frame(maxWidth: .infinity, alignment: .top)
    }
}

/// The panels floating in windows of their own, each where it was left, at the floating windows'
/// size: over the canvas (which takes no touch under them), under the panels hanging from the bar;
/// folded, their tabs. `screen` is the screen in the interface's points; the room they go in is
/// told to the layout by CanvasScreen.
struct FloatingPanelWindows: View {
    let model: CanvasModel
    let screen: CGSize

    /// The panels drawn here: the Layers panel and the library, the Actions panel and the
    /// Adjustments menu (the colour panel, the toolbars and an adjustment's controls float where
    /// they are drawn).
    private static let drawn: Set<LayoutPiece> = [.layers, .brushes, .actions, .adjustments]

    var body: some View {
        ZStack(alignment: .topLeading) {
            // drawn as they were last pressed, those pinned over the rest
            ForEach(model.pieceOrder().filter { Self.drawn.contains($0) }, id: \.self) { piece in
                if let window = model.shownPiece(piece) {
                    if window.folded {
                        FloatingTab(
                            shown: window,
                            title: pieceTitle(piece),
                            onUnfold: { model.foldPiece(piece, false) },
                            onDrag: { by in
                                if model.panelLayout.holding() != piece {
                                    model.holdPiece(piece, sizing: false)
                                }
                                model.dragPiece(by: by)
                            },
                            onLetGo: { model.letGoPiece() }
                        )
                    } else if piece == .layers || piece == .brushes {
                        FloatingPanelWindow(model: model, panel: piece == .layers ? .layers : .brushes, window: window)
                    } else {
                        FloatingPopoverWindow(model: model, piece: piece, window: window)
                    }
                }
            }
        }
        .frame(width: screen.width, height: screen.height, alignment: .topLeading)
    }
}

/// A floating piece's name, on its tab.
func pieceTitle(_ piece: LayoutPiece) -> String {
    switch piece {
    case .layers: "Layers"
    case .brushes: "Brush Library"
    case .colours: "Colors"
    case .actions: "Actions"
    case .adjustments: "Adjustments"
    case .adjusting: "Adjustment"
    case .selection: "Selection"
    case .transform: "Transform"
    case .liquify: "Liquify"
    case .reference: "Reference"
    }
}

/// The Actions panel or the Adjustments menu floating where the layout has it, at the floating
/// windows' size: its own popover draws as the window (see `Popover`).
private struct FloatingPopoverWindow: View {
    let model: CanvasModel
    let piece: LayoutPiece
    let window: PieceShown

    var body: some View {
        let sizable = pieceSizes(piece: piece).sizable
        let frame = FloatingFrame(
            width: CGFloat(window.width),
            height: CGFloat(window.height),
            onHold: { model.holdPiece(piece, sizing: $0) },
            onDrag: { model.dragPiece(by: $0) },
            onLetGo: { model.letGoPiece() },
            onClose: { model.closePiece(piece) },
            menu: windowMenu(model, piece)
        )
        ZStack(alignment: .bottomTrailing) {
            Group {
                switch piece {
                case .actions: ActionsPanel(model: model, arrowX: 0, maxHeight: frame.height)
                default: AdjustmentsPanel(model: model, arrowX: 0)
                }
            }
            .environment(\.floatWindow, FloatWindow(frame: frame, sizable: sizable))
            if sizable {
                FloatingCorner(frame: frame)
            }
        }
        .frame(width: frame.width, height: frame.height, alignment: .top)
        // the canvas under it takes no touch
        .contentShape(Rectangle())
        .onTapGesture {}
        .scaleEffect(CGFloat(window.scale), anchor: .topLeading)
        .offset(x: CGFloat(window.x), y: CGFloat(window.y))
    }
}

/// The grabber over a piece that isn't the Layers panel or the library (the Actions panel and the
/// Adjustments menu where they hang, a toolbar or an adjustment's controls at the bottom), while a
/// plugin lets it float: dragged out past `panelUndockDrag`, its window follows the finger, its
/// strip's middle under it; let go, it floats there.
struct PieceGrabber: View {
    let model: CanvasModel
    let piece: LayoutPiece

    var body: some View {
        ZStack(alignment: .top) {
            Capsule()
                .fill(Pc.grabber)
                .frame(width: 40, height: 4)
                .padding(.top, 4)
                .allowsHitTesting(false)
            Color.clear
                .frame(width: 100, height: 30)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 1, coordinateSpace: .global)
                        .onChanged { drag in
                            guard model.floating.undockingPiece == piece || hypot(drag.translation.width, drag.translation.height) > panelUndockDrag else { return }
                            let k = CGFloat(model.floating.scale)
                            model.undockPiece(piece, at: Interface.shared.point(drag.location), grab: floatingTopHeight / 2 / k)
                        }
                        .onEnded { _ in model.undockedPiece(piece) }
                )
        }
        .frame(width: 100, height: 30)
    }
}

/// Where each piece floating from the bottom shows where it always is, as measured (the window's
/// points as global frames give them, interface points once read).
struct PieceFrameKey: PreferenceKey {
    static var defaultValue: [LayoutPiece: CGRect] { [:] }

    static func reduce(value: inout [LayoutPiece: CGRect], nextValue: () -> [LayoutPiece: CGRect]) {
        value.merge(nextValue()) { $1 }
    }
}

extension View {
    /// Tells where this, `piece`'s plate, shows: `lift` above where it is laid out (a toolbar and an
    /// adjustment's panel are drawn 25 above their place).
    func reportsPiece(_ piece: LayoutPiece, lift: CGFloat = 0) -> some View {
        background(
            GeometryReader { g in
                Color.clear.preference(key: PieceFrameKey.self, value: [piece: Interface.shared.rect(g.frame(in: .global)).offsetBy(dx: 0, dy: -lift)])
            }
        )
    }
}

/// The tallest a toolbar or an adjustment's controls are: what measures taller is a menu of theirs.
private let mostBottomHeight: CGFloat = 400

/// A toolbar or an adjustment's controls, `piece` in the layout, drawn in a layer the size of the
/// screen (the menus it opens in it too): where it always is at the bottom, with a grabber over it
/// while a plugin lets it float; or, torn off, the whole layer moved so that it shows where the
/// layout has it, a strip over it to move it (its menu, its fold and its Close on it). It floats
/// while `visible`; its plate tells where it is (`reportsPiece`).
struct FloatsAtBottom: ViewModifier {
    let model: CanvasModel
    let piece: LayoutPiece
    let visible: Bool
    /// Where it shows where it always is (interface points), as last measured.
    @State private var home: CGRect?

    func body(content: Content) -> some View {
        let shown = visible ? model.shownPiece(piece) : nil
        let floating = shown.flatMap { $0.folded ? nil : $0 }
        let shift: CGSize = if let floating, let home {
            CGSize(width: CGFloat(floating.x) - home.minX, height: CGFloat(floating.y) + floatingTopHeight - home.minY)
        } else {
            .zero
        }
        let k = CGFloat(model.floating.scale)
        content
            .opacity(shown?.folded == true || (floating != nil && home == nil) ? 0 : 1)
            .offset(shift)
            .onPreferenceChange(PieceFrameKey.self) { frames in
                guard let f = frames[piece] else { return }
                // where it is drawn, less the shift: where it always is
                let at = f.offsetBy(dx: -shift.width, dy: -shift.height)
                guard at.height <= mostBottomHeight else { return }
                if home != at {
                    home = at
                }
                // the layout told its size at the floating windows' size (it shows at its own)
                model.tellPieceSize(piece, width: at.width / k, height: (at.height + floatingTopHeight) / k)
            }
            .overlay(alignment: .topLeading) {
                if let home, visible {
                    if let floating {
                        FloatingTopStrip(
                            frame: FloatingFrame(
                                width: home.width,
                                height: floatingTopHeight,
                                onHold: { model.holdPiece(piece, sizing: $0) },
                                onDrag: { model.dragPiece(by: $0) },
                                onLetGo: { model.letGoPiece() },
                                onClose: { model.closePiece(piece) },
                                menu: windowMenu(model, piece)
                            )
                        )
                        .frame(width: home.width)
                        .background(UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12, style: .circular).fill(Pc.popoverFill))
                        .offset(x: CGFloat(floating.x), y: CGFloat(floating.y))
                    } else if shown == nil, model.mayFloat(piece) {
                        PieceGrabber(model: model, piece: piece)
                            .offset(x: home.midX - 50, y: home.minY - 30)
                    }
                }
                if let tab = shown, tab.folded {
                    FloatingTab(
                        shown: tab,
                        title: pieceTitle(piece),
                        onUnfold: { model.foldPiece(piece, false) },
                        onDrag: { by in
                            if model.panelLayout.holding() != piece {
                                model.holdPiece(piece, sizing: false)
                            }
                            model.dragPiece(by: by)
                        },
                        onLetGo: { model.letGoPiece() }
                    )
                }
            }
            // shown again as its tool opens, if its window was hidden
            .onChange(of: visible) { _, now in
                if now {
                    model.hidePiece(piece, false)
                }
            }
    }
}

/// One panel's window: laid out at its own size, shown as big as the layout says, at its place.
private struct FloatingPanelWindow: View {
    let model: CanvasModel
    let panel: FloatingPanel
    let window: PieceShown

    var body: some View {
        let piece = model.piece(panel)
        let folds = model.layoutLook.on && model.layoutLook.fold
        let frame = FloatingFrame(
            width: CGFloat(window.width),
            height: CGFloat(window.height),
            onHold: { model.holdPiece(piece, sizing: $0) },
            onDrag: { model.dragPiece(by: $0) },
            onLetGo: { model.letGoPiece() },
            onClose: { model.closeFloating(panel) },
            onFold: folds ? { model.foldPiece(piece, true) } : nil,
            menu: windowMenu(model, piece)
        )
        ZStack(alignment: .bottomTrailing) {
            switch panel {
            case .layers:
                LayersPanel(model: model, arrowX: 0, maxHeight: frame.height, floating: frame)
                    // what shows when it opens: the layers' thumbnails
                    .onAppear { model.refreshThumbnails() }
            case .brushes:
                BrushLibrary(model: model, arrowX: 0, origin: .zero, screen: CGSize(width: frame.width, height: frame.height), floating: frame)
            }
            FloatingCorner(frame: frame)
        }
        .frame(width: frame.width, height: frame.height, alignment: .top)
        // the canvas under it takes no touch
        .contentShape(Rectangle())
        .onTapGesture {}
        .scaleEffect(CGFloat(window.scale), anchor: .topLeading)
        .offset(x: CGFloat(window.x), y: CGFloat(window.y))
    }
}
