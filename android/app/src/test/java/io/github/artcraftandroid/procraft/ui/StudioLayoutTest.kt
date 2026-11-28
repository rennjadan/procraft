package io.github.artcraftandroid.procraft.ui

import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** How the brush studio lays itself out by the window it has: side by side, or the rail over the pad. */
class StudioLayoutTest {
    /** The room the pad beside the pages and settings has in a window [width] wide. */
    private fun padBeside(width: Dp) = width - PagesAndSettings

    @Test
    fun theStudioTakesTheMeasuredWidthsAndThePadsButtonsTheirs() {
        // the pages' card with its margin, the settings and the line after them
        assertEquals(654.dp, PagesAndSettings)
        // 21 in, the Drawing Pad button, a gap, Cancel, a gap, Done, 20 in
        assertEquals(329.dp, PadLeast)
    }

    @Test
    fun tabletsOnTheirSideLayTheirPagesSettingsAndPadSideBySide() {
        // (width, height): the reference's own screen, a tablet of the reporter's size (#411, about 1006
        // across, which the rail had been given since the phone fix, as v0.0.7 had not), common tablets
        val sideways = listOf(
            1133.dp to 744.dp,
            1006.dp to 603.dp,
            1005.dp to 603.dp,
            1280.dp to 800.dp,
            1340.dp to 800.dp,
            1024.dp to 600.dp,
            1366.dp to 1024.dp,
            2000.dp to 1200.dp,
        )
        for ((w, h) in sideways) {
            assertTrue("$w x $h", studioSideBySide(w, h))
            // and the pad beside them has room for its buttons
            assertTrue("$w x $h", padBeside(w) >= PadLeast)
        }
        // the least that does
        assertTrue(studioSideBySide(983.dp, 600.dp))
        assertFalse(studioSideBySide(982.dp, 600.dp))
    }

    @Test
    fun aPhoneOnItsSideOrAHalfWindowTakesTheRailSoDoneStays() {
        // #318: on a phone held sideways the pad had about 80 across, and Done was squeezed to nothing
        for ((w, h) in listOf(640.dp to 360.dp, 732.dp to 412.dp, 780.dp to 360.dp, 850.dp to 400.dp, 915.dp to 412.dp, 960.dp to 600.dp)) {
            assertFalse("$w x $h", studioSideBySide(w, h))
            assertTrue("$w x $h", padBeside(w) < PadLeast)
        }
    }

    @Test
    fun anUprightWindowAlwaysTakesTheRailWhateverTheScreensTurn() {
        // upright tablets, and a landscape screen's split window that is taller than wide
        for ((w, h) in listOf(744.dp to 1133.dp, 800.dp to 1280.dp, 603.dp to 1006.dp, 1024.dp to 1366.dp, 1200.dp to 1300.dp, 990.dp to 990.dp)) {
            assertFalse("$w x $h", studioSideBySide(w, h))
        }
    }
}
