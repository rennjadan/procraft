package io.github.artcraftandroid.procraft.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalContext
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.currentCompositionLocalContext
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.round
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties

/**
 * The menus panels draw in the app's window itself, over everything else in it, rather than each in a
 * popup window of its own: no window has to come up (and be laid out and drawn a first time) before such
 * a menu shows, so it shows in the frame it is opened. A panel puts its menus here ([MenuLayerContent]);
 * [MenuLayerHost], last over the screen, draws them.
 */
@Stable
class MenuLayer {
    internal val entries = mutableStateListOf<MenuLayerEntry>()
}

/** What a panel put in the [MenuLayer]: what draws its menus, and the composition locals where it put it. */
internal class MenuLayerEntry {
    var content by mutableStateOf<(@Composable () -> Unit)?>(null)
    var locals by mutableStateOf<CompositionLocalContext?>(null)
}

/** The window's menu layer, where there is one (the canvas screen's). */
val LocalMenuLayer = staticCompositionLocalOf<MenuLayer?> { null }

/** Whether what is composed is in the menu layer ([MenuLayerHost]) rather than where it was put. */
private val LocalInMenuLayer = staticCompositionLocalOf { false }

/** Draws [layer]'s menus, as big as the window: put last, over everything else. */
@Composable
fun MenuLayerHost(layer: MenuLayer) {
    Box(Modifier.fillMaxSize()) {
        for (entry in layer.entries) {
            key(entry) {
                val content = entry.content
                val locals = entry.locals
                if (content != null && locals != null) {
                    CompositionLocalProvider(locals) {
                        CompositionLocalProvider(LocalInMenuLayer provides true, content = content)
                    }
                }
            }
        }
    }
}

/**
 * [content] composed in the window's [MenuLayer] with this place's composition locals, so that the menus
 * it draws ([WindowMenu]) lie over everything and no panel cuts them off. It is read there as it is here:
 * a menu opened by a change of state shows in the frame the change is drawn. With no layer, it is
 * composed here.
 */
@Composable
fun MenuLayerContent(content: @Composable () -> Unit) {
    val layer = LocalMenuLayer.current
    if (layer == null) {
        content()
        return
    }
    val locals = currentCompositionLocalContext
    val entry = remember { MenuLayerEntry() }
    SideEffect {
        entry.content = content
        entry.locals = locals
    }
    DisposableEffect(layer, entry) {
        layer.entries.add(entry)
        onDispose { layer.entries.remove(entry) }
    }
}

/**
 * Where a [WindowMenu] hangs from: what [Modifier.menuAnchor] is put on, as it is laid out. A menu
 * hanging from it follows it as it moves (a list scrolling under the menu), as a popup follows the layout
 * it is put up in.
 */
@Stable
class MenuAnchor {
    internal var coordinates: LayoutCoordinates? = null

    // counted up as it moves while a menu hangs from it ([followed]), for the menu to be placed again
    internal var moves by mutableIntStateOf(0)
    internal var followed = false

    /** Its bounds in the window, as a popup's anchor's (whole, not cut by what it is in), while laid out. */
    internal fun bounds(): IntRect? = coordinates?.takeIf { it.isAttached }?.let { IntRect(it.positionInWindow().round(), it.size) }

    /** [bounds], read so that what reads them (a menu's placing) runs again as it moves while followed. */
    internal fun followedBounds(): IntRect? = if (moves >= 0) bounds() else null
}

/** Makes this what [anchor] stands for. */
fun Modifier.menuAnchor(anchor: MenuAnchor): Modifier = onGloballyPositioned {
    anchor.coordinates = it
    if (anchor.followed) anchor.moves++
}

/**
 * How long a frame drawn now takes to reach the screen, milliseconds: two of the display's refresh
 * periods (on the Legion at 60 Hz a menu's first frame showed 33 ms behind its time). A menu meant
 * to be seen at a time opens from this much before it.
 */
@Composable
fun rememberFrameLag(): Long {
    val view = LocalView.current
    return remember(view) { (2000f / (view.display?.refreshRate?.takeIf { it > 1f } ?: 60f)).toLong() }
}

/** [place] from [anchor]'s bounds (when it has them) rather than those of the layout a popup is put up in. */
internal class AnchoredPlace(private val anchor: MenuAnchor, private val place: PopupPositionProvider) : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize): IntOffset =
        place.calculatePosition(anchor.bounds() ?: anchorBounds, windowSize, layoutDirection, popupContentSize)
}

/**
 * A menu up while [open], and while it closes ([MenuPresence], given [askedAt]), placed by [place] from
 * [anchor], as a popup would be; with no anchor (a row gone from its list) there is none, as a popup put
 * up in it went with it. In the menu layer ([MenuLayerContent]) it is drawn in the window itself, over
 * everything; elsewhere it is a popup. Either way it acts as a focused popup does: the first touch
 * anywhere off it is taken whole and only closes it ([onDismiss]), as Back does. [content] is given the
 * menu's look, and is composed as a popup's is, its text scaled by [textScale]. [ahead]: not open yet,
 * but composed already for an [askedAt] still to come, unseen until then and taking no touch (so that
 * its first frame is drawn then, not a frame or two after it opens).
 */
@Composable
internal fun WindowMenu(
    open: Boolean,
    anchor: MenuAnchor?,
    origin: TransformOrigin,
    place: PopupPositionProvider,
    onDismiss: () -> Unit,
    askedAt: Long = 0L,
    textScale: Float = 1f,
    ahead: Boolean = false,
    content: @Composable (Modifier) -> Unit,
) {
    if (anchor == null) return
    anchor.followed = open || ahead
    // opened this time, not only composed ahead: one composed ahead and let go before it opens goes at
    // once (unseen, or as good as: no closing for a press that was a tap)
    val opened = remember { booleanArrayOf(false) }
    if (open) opened[0] = true else if (ahead) opened[0] = false
    if (!open && !ahead && !opened[0]) return
    if (!LocalInMenuLayer.current) {
        if (ahead && !open) return
        MenuPresence(open, origin, askedAt) { look ->
            PcPopup(AnchoredPlace(anchor, place), onDismiss, PopupProperties(focusable = true), textScale) { content(look) }
        }
        return
    }
    val dismiss by rememberUpdatedState(onDismiss)
    val view = LocalView.current
    MenuPresence(open || ahead, origin, askedAt) { look ->
        BackHandler(open) { dismiss() }
        // off the menu, over the whole window (once it is open)
        if (!ahead || open) Box(
            Modifier.fillMaxSize().pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false).consume()
                    dismiss()
                    do {
                        val event = awaitPointerEvent()
                        event.changes.forEach { it.consume() }
                    } while (event.changes.any { it.pressed })
                }
            },
        )
        PopupLocals(textScale) {
            Layout({ content(look.keepsItsTouches()) }, Modifier.fillMaxSize()) { measurables, constraints ->
                val menu = measurables.firstOrNull()?.measure(Constraints(maxWidth = constraints.maxWidth, maxHeight = constraints.maxHeight))
                layout(constraints.maxWidth, constraints.maxHeight) {
                    if (menu == null) return@layout
                    val bounds = anchor.followedBounds() ?: return@layout
                    // in the window's visible frame, as a popup is
                    val frame = android.graphics.Rect().also(view::getWindowVisibleDisplayFrame)
                    val at = place.calculatePosition(bounds, IntSize(frame.width(), frame.height()), layoutDirection, IntSize(menu.width, menu.height))
                    menu.place(at - (coordinates?.positionInWindow()?.round() ?: IntOffset.Zero))
                }
            }
        }
    }
}
