import SwiftUI
import UniformTypeIdentifiers

/// A library's card: as tall as measured, how far apart, its corners, a set's row and a brush's.
private let cardHeight: CGFloat = 490
private let cardGap: CGFloat = 8
private let cardCorner: CGFloat = 16
/// A card is the library as its panel shows it, this much smaller (as measured: its set cells 22.9
/// tall, 28.3 apart), its sets starting this far below the card's top, under its name.
private let cardScale: CGFloat = 0.457
private let cardSetsTop: CGFloat = 27
/// The sets and brushes a card has room for under its name: any more would only be cut off.
private let cardSetsShown = 17
private let cardBrushesShown = 12
/// Menus stay below the top bar.
private let librariesMenuTop: CGFloat = 57
/// The space the cards report their places in, for a card's menu.
private let librariesSpace = "libraries"

/// Where a library's title menu hangs in its panel, as measured: 8 in from the panel's left, its
/// top 54 below the panel's (the header starting under the arrow).
let libraryTitleMenuAt = CGPoint(x: 8, y: Pc.arrowHeight + 54)

/// `p`, a menu `size` big's place in a panel whose top left corner is `origin` on a screen
/// `screen` big, moved as little as keeps the menu on the screen and its top at least `top` down it.
private func keptOnScreen(_ p: CGPoint, _ size: CGSize, origin: CGPoint, screen: CGSize, below top: CGFloat) -> CGPoint {
    CGPoint(
        x: min(max(origin.x + p.x, 0), max(screen.width - size.width, 0)) - origin.x,
        y: min(max(origin.y + p.y, top), max(screen.height - size.height, top)) - origin.y
    )
}

/// A library's title menu: rename it, share it, or go back to every library's card.
struct LibraryTitleMenu: View {
    let model: CanvasModel
    let close: () -> Void

    /// Two rows, a line 11 clear, and the last row.
    static let height: CGFloat = 14 + 80 + 23 + 40

    var body: some View {
        GlassMenu(overPanel: true) {
            MenuRow(icon: PcIcons.menuRename, label: "Rename") {
                close()
                model.renamingLibrary = model.libraryPlace
            }
            MenuRow(icon: PcIcons.menuShare, label: "Share") {
                close()
                model.shareLibrary()
            }
            MenuLine(gap: 11)
            MenuRow(icon: PcIcons.menuLibraries, label: "Back to libraries") {
                close()
                model.showingLibraries = true
            }
        }
    }
}

/// What the libraries' panel has open: its title's and plus's menus, a card's menu, the file
/// picker, and where the cards are.
@Observable
private final class LibrariesUi {
    var titleMenu = false
    var adding = false
    var cardMenu: Int?
    var importing = false
    var cardFrames: [Int: CGRect] = [:]
}

private struct CardFrames: PreferenceKey {
    static let defaultValue: [Int: CGRect] = [:]
    static func reduce(value: inout [Int: CGRect], nextValue: () -> [Int: CGRect]) {
        value.merge(nextValue()) { $1 }
    }
}

/// The library panel showing every library's card (tapped, that library is brought into use;
/// held, its menu), with the plus for a new one or one from a file and the title's menu to put the
/// one the app comes with back; or a library with no sets yet, with the plus to make one.
struct LibrariesPanel: View {
    let model: CanvasModel
    let arrowX: CGFloat
    /// The panel's top left corner on the screen, and the screen's size: the menus are kept on it.
    let origin: CGPoint
    let screen: CGSize
    /// Floating in a window of its own, as the library it shows in place of: its frame.
    var floating: FloatingFrame?
    @State private var ui = LibrariesUi()

    var body: some View {
        let showing = model.showingLibraries
        let renaming = !showing && model.renamingLibrary == model.libraryPlace
        Popover(width: floating?.width ?? brushLibraryWidth, arrowX: arrowX, side: floating == nil ? .top : .none) {
            if let floating {
                FloatingTopStrip(frame: floating)
            }
            PanelHeader(
                title: showing ? "Brush Libraries" : model.libraryName, start: 20, end: 12, height: 52,
                onTitle: { withAnimation(Motion.menuOpen) { ui.titleMenu = true } },
                rename: renaming ? { model.renameLibrary(model.libraryPlace, name: $0) } : nil
            ) {
                PcIconView(PcIcons.plusMedium, size: 41, color: Pc.panelGlyph)
                    .frame(width: 41, height: 40)
                    .contentShape(Rectangle())
                    .onTapGesture { withAnimation(Motion.menuOpen) { ui.adding = true } }
                    .padding(.top, 12)
            }
            if showing {
                LibraryCards(model: model, ui: ui)
            } else {
                EmptyLibrary()
            }
            if floating != nil {
                Spacer(minLength: 0)
                FloatingBottomStrip()
            }
        }
        .coordinateSpace(.named(librariesSpace))
        .onPreferenceChange(CardFrames.self) { ui.cardFrames = $0 }
        .overlay(alignment: .topLeading) { LibrariesMenus(model: model, ui: ui, origin: origin, screen: screen) }
        .fileImporter(isPresented: $ui.importing, allowedContentTypes: [.item]) { result in
            if case let .success(url) = result {
                model.importBrushes(from: url)
            }
        }
        #if DEBUG
        .onAppear {
            switch ScreenCapture.libraryShows {
            case "libraries-title":
                ui.titleMenu = true
            case "libraries-add":
                ui.adding = true
            case "libraries-card-menu":
                ui.cardMenu = 0
            default:
                break
            }
        }
        #endif
    }
}

/// A library with no sets yet: what to do about it.
private struct EmptyLibrary: View {
    var body: some View {
        VStack(spacing: 0) {
            Text(verbatim: "This library has no brushes yet")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(Pc.panelTitle)
                .lineLimit(2)
                .multilineTextAlignment(.center)
            Text(verbatim: "Make a set with +, then brushes in it, or bring brushes in from your files.")
                .font(.system(size: 14))
                .foregroundStyle(Pc.panelGlyph)
                .multilineTextAlignment(.center)
                .padding(.top, 8)
        }
        .padding(.horizontal, 32)
        .padding(.vertical, 48)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }
}

/// Every library's card, two to a row.
private struct LibraryCards: View {
    let model: CanvasModel
    let ui: LibrariesUi

    var body: some View {
        let columns = [GridItem(.flexible(), spacing: cardGap), GridItem(.flexible(), spacing: cardGap)]
        ScrollView {
            LazyVGrid(columns: columns, spacing: cardGap) {
                ForEach(Array(model.brushLibraries.enumerated()), id: \.offset) { i, library in
                    LibraryCard(model: model, ui: ui, index: i, library: library)
                }
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 12)
        }
        .scrollIndicators(.hidden)
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

/// A library's card: its name over a small likeness of it, its sets down the left and the brushes
/// of one of them beside them; the one in use outlined in blue.
private struct LibraryCard: View {
    let model: CanvasModel
    let ui: LibrariesUi
    let index: Int
    let library: BrushLibraryInfo
    /// Two fingers pinching out on it: how far (their spread, a part of what it was).
    @State private var zoom: CGFloat = 1

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: cardCorner, style: .circular)
        // the one in use's shown set; another's first with a brush chosen in it
        let chosen = library.current ? model.shownSet : (library.sets.firstIndex { $0.chosen != nil } ?? 0)
        let k = cardScale
        ZStack(alignment: .topLeading) {
            // the library's lists, small: the sets (Recent first) and the shown one's brushes
            HStack(alignment: .top, spacing: 0) {
                CardSets(model: model, sets: library.sets, chosen: chosen, recentLit: library.current && model.shownSet == CanvasModel.recentSet)
                    .frame(width: 168 * k, alignment: .top)
                CardBrushes(
                    model: model, library: index, current: library.current,
                    set: library.sets.indices.contains(chosen) ? library.sets[chosen] : nil,
                    tint: model.previewTints().other
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(Pc.brushColumn)
            }
            .padding(.top, cardSetsTop - 12 * k)
            .frame(maxHeight: .infinity, alignment: .top)
            // its name over them, its middle 14.5 down
            Group {
                if model.renamingLibrary == index {
                    InlineName(name: library.name, color: Pc.panelTitle, fontSize: 14) { model.renameLibrary(index, name: $0) }
                } else {
                    Text(verbatim: library.name)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(Pc.panelTitle)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
            .padding(.leading, 9)
            .padding(.trailing, 9)
            .frame(height: 29)
        }
        .frame(maxWidth: .infinity, minHeight: cardHeight, maxHeight: cardHeight, alignment: .topLeading)
        .background(shape.fill(Pc.setCell))
        .overlay {
            if library.current {
                shape.strokeBorder(Pc.accent, lineWidth: 2)
            }
        }
        .clipShape(shape)
        .contentShape(shape)
        // pinched out, it grows with the fingers, and let go far enough apart it opens
        .scaleEffect(min(max(zoom, 1), pinchToLibrary))
        .zIndex(zoom > 1 ? 1 : 0)
        .simultaneousGesture(
            MagnifyGesture()
                .onChanged { zoom = $0.magnification }
                .onEnded { value in
                    withAnimation(Motion.swipeSettle) { zoom = 1 }
                    if value.magnification > pinchToLibrary {
                        model.openLibrary(index)
                    }
                }
        )
        .onTapGesture { model.openLibrary(index) }
        .onLongPressGesture(minimumDuration: 0.5, maximumDistance: 10) {
            withAnimation(Motion.menuOpen) { ui.cardMenu = index }
        }
        .background(GeometryReader { g in
            Color.clear.preference(key: CardFrames.self, value: [index: g.frame(in: .named(librariesSpace))])
        })
    }
}

/// A card's sets, as the library's column shows them small: Recent, then the sets, the one whose
/// brushes it shows lit.
private struct CardSets: View {
    let model: CanvasModel
    let sets: [BrushSetInfo]
    let chosen: Int
    let recentLit: Bool

    var body: some View {
        let k = cardScale
        VStack(spacing: 12 * k) {
            CardSetRow(icon: PcIcons.recentSet, name: "Recent", lit: recentLit)
            ForEach(Array(sets.prefix(cardSetsShown).enumerated()), id: \.offset) { s, set in
                CardSetRow(icon: model.setIcon(set), name: set.name, lit: !recentLit && s == chosen)
            }
        }
        .padding(12 * k)
    }
}

/// A set's cell, small: its icon and name on a rounded cell, blue when lit.
private struct CardSetRow: View {
    let icon: PcIcon
    let name: String
    let lit: Bool

    var body: some View {
        let k = cardScale
        let ink = lit ? Color.white : Pc.panelTitle
        HStack(spacing: 0) {
            PcIconView(icon, size: 40 * k, color: ink)
                .padding(.leading, 4 * k)
            Text(verbatim: name)
                .font(.system(size: 14 * k, weight: .semibold))
                .foregroundStyle(ink)
                .lineLimit(2)
                .truncationMode(.middle)
                .padding(.leading, 1 * k)
                .padding(.trailing, 8 * k)
            Spacer(minLength: 0)
        }
        .frame(height: 50 * k)
        .background(RoundedRectangle(cornerRadius: 12 * k, style: .circular).fill(lit ? Pc.rowSelected : Pc.setCell))
    }
}

/// A card's brushes, as the library's column shows them small: each its name over its stroke, the
/// set's lit one blue.
private struct CardBrushes: View {
    let model: CanvasModel
    let library: Int
    /// Whether the card is the library in use's (its lit brush the one the library lights).
    let current: Bool
    let set: BrushSetInfo?
    /// The colour a plugin gives the sample strokes, if one does.
    let tint: Color?

    var body: some View {
        let k = cardScale
        VStack(spacing: 8 * k) {
            ForEach(Array((set?.brushes ?? []).prefix(cardBrushesShown).enumerated()), id: \.offset) { _, b in
                let lit = current ? model.highlighted(b) : set?.chosen == b.index
                CardBrush(model: model, library: library, brush: b, lit: lit, tint: lit ? nil : tint)
            }
        }
        .padding(.horizontal, 12 * k)
        .padding(.top, 9.75 * k)
    }
}

private struct CardBrush: View {
    let model: CanvasModel
    let library: Int
    let brush: BrushInfo
    let lit: Bool
    let tint: Color?

    var body: some View {
        let k = cardScale
        let key = "\(library)-\(brush.set)-\(brush.index)"
        let stroke = model.cardPreviews[key]
        let missing = stroke == nil
        let shape = RoundedRectangle(cornerRadius: 12 * k, style: .circular)
        ZStack(alignment: .topLeading) {
            shape.fill(lit ? Pc.rowSelected : Color.clear)
            if let stroke {
                // filling the cell's height, centred, as the library's strokes do; in the colour a
                // plugin gives them, if one does
                SampleStroke(image: stroke, tint: tint)
            }
            Text(verbatim: brush.name)
                .font(.system(size: 14 * k, weight: .semibold))
                .foregroundStyle(lit ? Color.white : Pc.panelTitle)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.leading, 18 * k)
                .padding(.top, 11.75 * k)
        }
        .frame(maxWidth: .infinity, minHeight: 80 * k, maxHeight: 80 * k, alignment: .topLeading)
        .clipShape(shape)
        // drawn once it shows, and again once the libraries are read again (they go by place)
        .task(id: "\(key) \(missing)") {
            if missing {
                model.loadCardPreview(library: library, set: brush.set, index: brush.index)
            }
        }
    }
}

/// The libraries' panel's menus: the title's, the plus's and a card's, each kept on the screen.
private struct LibrariesMenus: View {
    let model: CanvasModel
    let ui: LibrariesUi
    let origin: CGPoint
    let screen: CGSize

    private func kept(_ p: CGPoint, _ size: CGSize) -> CGPoint {
        keptOnScreen(p, size, origin: origin, screen: screen, below: librariesMenuTop)
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            if ui.titleMenu {
                MenuDismissal { closeAll() }
                titleMenu
            }
            if ui.adding {
                MenuDismissal { closeAll() }
                addMenu
            }
            if let i = ui.cardMenu, let frame = ui.cardFrames[i], model.brushLibraries.indices.contains(i) {
                MenuDismissal { closeAll() }
                cardMenu(i, frame: frame)
            }
        }
    }

    private func closeAll() {
        withAnimation(Motion.menuClose) {
            ui.titleMenu = false
            ui.adding = false
            ui.cardMenu = nil
        }
    }

    /// Every library's card: the one the app comes with put back; one library: its own menu.
    @ViewBuilder
    private var titleMenu: some View {
        if model.showingLibraries {
            let at = kept(libraryTitleMenuAt, CGSize(width: glassMenuWidth, height: 14 + 40))
            GlassMenu(overPanel: true) {
                MenuRow(icon: PcIcons.menuRestore, label: "Restore default library") {
                    closeAll()
                    model.restoreLibraries()
                }
            }
            .offset(x: at.x, y: at.y)
            .menuTransition(.topLeading)
        } else {
            let at = kept(libraryTitleMenuAt, CGSize(width: glassMenuWidth, height: LibraryTitleMenu.height))
            LibraryTitleMenu(model: model) { closeAll() }
                .offset(x: at.x, y: at.y)
                .menuTransition(.topLeading)
        }
    }

    /// Hanging from the plus: a new library or one from a file; or, in a library with no sets, a new
    /// set or brushes from a file.
    private var addMenu: some View {
        let showing = model.showingLibraries
        let height: CGFloat = showing ? 14 + 80 : 14 + 40 + 23 + 40
        // the plus's lower right corner: 12 in from the header's right, its 40 under the header's
        // first 12 (the header starting under the arrow)
        let plus = CGPoint(x: brushLibraryWidth - 12, y: Pc.arrowHeight + 12 + 40)
        let spot = kept(CGPoint(x: plus.x - glassMenuWidth, y: plus.y), CGSize(width: glassMenuWidth, height: height))
        return GlassMenu(overPanel: true) {
            if showing {
                MenuRow(icon: PcIcons.menuNewLibrary, label: "Create new library") {
                    closeAll()
                    model.newLibrary()
                }
                MenuRow(icon: PcIcons.menuFolder, label: "Import from Files") {
                    closeAll()
                    ui.importing = true
                }
            } else {
                MenuRow(icon: PcIcons.menuNewSet, label: "Create new set") {
                    closeAll()
                    model.newBrushSet()
                }
                MenuLine(gap: 11)
                MenuRow(icon: PcIcons.menuFolder, label: "Import from Files") {
                    closeAll()
                    ui.importing = true
                }
            }
        }
        .offset(x: spot.x, y: spot.y)
        .menuTransition(.topTrailing)
    }

    /// Over a card held, centred on it a quarter of the way down: rename, duplicate or delete it
    /// (not the only one).
    private func cardMenu(_ i: Int, frame: CGRect) -> some View {
        let canDelete = model.brushLibraries.count > 1
        let height: CGFloat = 14 + 80 + (canDelete ? 23 + 40 : 0)
        let at = kept(CGPoint(x: frame.midX - glassMenuWidth / 2, y: frame.minY + frame.height / 4), CGSize(width: glassMenuWidth, height: height))
        return GlassMenu {
            MenuRow(icon: PcIcons.menuRename, label: "Rename") {
                closeAll()
                model.renamingLibrary = i
            }
            MenuRow(icon: PcIcons.menuDuplicate, label: "Duplicate") {
                closeAll()
                model.duplicateLibrary(i)
            }
            if canDelete {
                MenuLine(gap: 11)
                MenuRow(icon: PcIcons.menuTrash, label: "Delete", ink: Pc.menuDanger) {
                    closeAll()
                    model.question = Question(title: "Delete this library?", message: "Its sets and brushes go with it.") {
                        model.deleteLibrary(i)
                    }
                }
            }
        }
        .offset(x: at.x, y: at.y)
        .menuTransition(.top)
    }
}
