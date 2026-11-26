package io.github.artcraftandroid.procraft.ui

import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import org.junit.Assert.assertEquals
import org.junit.Test

class MenuLayerTest {
    @Test
    fun aMenuHangsFromItsAnchorAsAPopupPutUpInItWould() {
        // a brush's menu: under the row, their left edges in line, kept under the top bar and in the window
        val brushMenu = MenuPlace(57) { a, _ -> IntOffset(a.left, a.bottom) }
        val window = IntSize(2560, 1600)
        val menu = IntSize(253, 300)
        val row = IntRect(1000, 400, 1400, 480)
        // with no anchor laid out, from the layout it is in (a popup's); with one, from it
        val unplaced = AnchoredPlace(MenuAnchor(), brushMenu)
        assertEquals(IntOffset(1000, 480), unplaced.calculatePosition(row, window, LayoutDirection.Ltr, menu))
        // low in the window it is kept in it; high, under the top bar
        assertEquals(IntOffset(1000, 1300), brushMenu.calculatePosition(IntRect(1000, 1400, 1400, 1480), window, LayoutDirection.Ltr, menu))
        assertEquals(IntOffset(1000, 57), brushMenu.calculatePosition(IntRect(1000, -200, 1400, -120), window, LayoutDirection.Ltr, menu))
        // a set's: left of it, 6 clear, centred on it
        val setMenu = MenuPlace(57) { a, size -> IntOffset(a.left - size.width - 6, a.center.y - size.height / 2) }
        assertEquals(IntOffset(1000 - 253 - 6, 440 - 150), setMenu.calculatePosition(row, window, LayoutDirection.Ltr, menu))
    }
}
