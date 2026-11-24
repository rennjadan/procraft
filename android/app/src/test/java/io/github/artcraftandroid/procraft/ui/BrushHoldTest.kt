package io.github.artcraftandroid.procraft.ui

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import io.github.artcraftandroid.procraft.core.BrushInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BrushHoldTest {
    // the sets down the left, the shown set's (set 1) brushes beside them, 90 tall each
    private val sets = mapOf(0 to Rect(0f, 0f, 200f, 60f), 1 to Rect(0f, 70f, 200f, 130f), 2 to Rect(0f, 140f, 200f, 200f))
    private val brushes = (0 until 5).map { (1 to it) to Rect(220f, it * 100f, 620f, it * 100f + 90f) }

    @Test
    fun aStillHoldShowsTheMenuAndLettingGoKeepsIt() {
        val hold = BrushHold(slop = 8f, movable = true)
        // a finger never quite still: nothing past the slop drags
        for (jitter in listOf(Offset(1f, 0f), Offset(0f, -2f), Offset(-3f, 4f), Offset(5.6f, 5.6f))) assertFalse(hold.move(jitter))
        assertFalse(hold.dragging)
    }

    @Test
    fun aHoldMovedOnDragsTheBrushAndLetGoReordersIt() {
        val hold = BrushHold(slop = 8f, movable = true)
        val took = Offset(400f, 145f)
        // held on the second brush, moved down: the drag starts once (the menu goes then)
        assertTrue(hold.move(Offset(0f, 9f)))
        assertTrue(hold.dragging)
        assertFalse(hold.move(Offset(0f, 200f)))
        // let go over the fourth brush: it takes that place
        assertEquals(1 to 3, brushDropPlace(1 to 1, took + Offset(0f, 200f), sets, brushes))
        // over another set: to its top; over its own set, or its own place, or nothing: stays
        assertEquals(2 to 0, brushDropPlace(1 to 1, Offset(100f, 150f), sets, brushes))
        assertNull(brushDropPlace(1 to 1, Offset(100f, 100f), sets, brushes))
        assertNull(brushDropPlace(1 to 1, took, sets, brushes))
        assertNull(brushDropPlace(1 to 1, Offset(700f, 145f), sets, brushes))
    }

    @Test
    fun theRecentSetsBrushesKeepTheirMenuAndStay() {
        val hold = BrushHold(slop = 8f, movable = false)
        assertFalse(hold.move(Offset(0f, 300f)))
        assertFalse(hold.dragging)
    }

    @Test
    fun brushesDraggedTogetherGoOntoAnotherSetOrAPlaceNotAmongThem() {
        val moving = listOf(1 to 1, 1 to 2)
        // over another set: to its top, a set closed or not; over their own: nowhere
        assertEquals(0 to 0, brushesDropPlace(moving, Offset(100f, 30f), sets, brushes))
        assertEquals(2 to 0, brushesDropPlace(moving, Offset(100f, 150f), sets, brushes))
        assertNull(brushesDropPlace(moving, Offset(100f, 100f), sets, brushes))
        // over a brush of theirs: nowhere; over another: its place
        assertNull(brushesDropPlace(moving, Offset(400f, 245f), sets, brushes))
        assertEquals(1 to 4, brushesDropPlace(moving, Offset(400f, 445f), sets, brushes))
        assertNull(brushesDropPlace(emptyList(), Offset(100f, 150f), sets, brushes))
    }

    @Test
    fun brushesHeldOverAnotherSetUntilItOpensGoAmongItsBrushes() {
        // set 2 opened under brushes of set 1: let go over one of its brushes, they go to its place
        // (the core copies them there, a different set from theirs)
        val opened = (0 until 3).map { (2 to it) to Rect(220f, it * 100f, 620f, it * 100f + 90f) }
        assertEquals(2 to 1, brushesDropPlace(listOf(1 to 0, 1 to 3), Offset(400f, 145f), sets, opened))
        assertEquals(2 to 1, brushDropPlace(1 to 0, Offset(400f, 145f), sets, opened))
        // it opens only after a hold as long as the original's (1.76 s measured)
        assertTrue(SPRING_LOAD_MS in 1600L..2000L)
    }

    @Test
    fun aPickedBrushHeldTakesThePickedOnesWithItInTheirOrder() {
        val set = (0 until 6).map { BrushInfo(1u, it.toUInt(), "Brush $it") }
        // not picked: it alone; picked, or the current brush while others are: all of them in order
        assertEquals(listOf(set[2]), draggedTogether(set[2], emptyList(), false, set))
        assertEquals(listOf(set[2]), draggedTogether(set[2], listOf("1-4"), false, set))
        assertEquals(listOf(set[1], set[4], set[5]), draggedTogether(set[4], listOf("1-5", "1-1", "1-4"), false, set))
        assertEquals(listOf(set[0], set[3]), draggedTogether(set[0], listOf("1-3"), true, set))
    }
}
