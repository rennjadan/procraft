package io.github.artcraftandroid.procraft.ui

import androidx.compose.animation.core.AnimationVector1D
import androidx.compose.animation.core.TweenSpec
import androidx.compose.animation.core.VectorConverter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MenuPresenceTest {
    /** The value a [TweenSpec] of the menu's (as the menus were animated before) gives [ms] in, from [from] to [to]. */
    private fun tween(easing: androidx.compose.animation.core.Easing, from: Float, to: Float, ms: Long): Float =
        TweenSpec<Float>(Motion.MENU_MS, 0, easing).vectorize(Float.VectorConverter)
            .getValueFromNanos(ms * 1_000_000L, AnimationVector1D(from), AnimationVector1D(to), AnimationVector1D(0f)).value

    @Test
    fun aMenuLooksFrameForFrameAsItsTweenOpeningAndClosing() {
        // the same look as the tweens the menus ran before, at every 60 Hz frame and past the end
        for (ms in 0L..320L step 16L) {
            val (a, k) = menuLook(true, 0f, Motion.MENU_START_SCALE, ms)
            assertEquals("alpha opening at $ms ms", tween(Motion.MenuOpen, 0f, 1f, ms), a, 1e-5f)
            assertEquals("scale opening at $ms ms", tween(Motion.MenuOpen, Motion.MENU_START_SCALE, 1f, ms), k, 1e-5f)
            // closing from part way open, and opening again from part way closed
            val (ca, ck) = menuLook(false, 0.8f, 0.9f, ms)
            assertEquals("alpha closing at $ms ms", tween(Motion.Settle, 0.8f, 0f, ms), ca, 1e-5f)
            assertEquals("scale closing at $ms ms", tween(Motion.Settle, 0.9f, Motion.MENU_END_SCALE, ms), ck, 1e-5f)
            val (ra, rk) = menuLook(true, 0.3f, 0.85f, ms)
            assertEquals(tween(Motion.MenuOpen, 0.3f, 1f, ms), ra, 1e-5f)
            assertEquals(tween(Motion.MenuOpen, 0.85f, 1f, ms), rk, 1e-5f)
        }
        // done: wholly shown, and wholly gone
        val (openA, openK) = menuLook(true, 0f, Motion.MENU_START_SCALE, Motion.MENU_MS.toLong())
        assertEquals(1f, openA, 1e-6f)
        assertEquals(1f, openK, 1e-6f)
        val (goneA, goneK) = menuLook(false, 1f, 1f, Motion.MENU_MS.toLong())
        assertEquals(0f, goneA, 1e-6f)
        assertEquals(Motion.MENU_END_SCALE, goneK, 1e-6f)
    }

    /**
     * Why a menu opens from when it was asked for: the tween began a frame after the menu was first
     * drawn, at nothing, so a menu was up two frames before any of it showed (its first frame at nothing,
     * the tween's first at nothing). Timed from the ask, the first frame drawn shows it.
     */
    @Test
    fun aMenuDrawnAFrameAfterItWasAskedForAlreadyShows() {
        // asked for (the hold decided) 5 ms before the frame it is first drawn in, then frames 16.7 ms apart
        val frames = listOf(5L, 22L, 39L)
        val shown = frames.map { menuLook(true, 0f, Motion.MENU_START_SCALE, it).first }
        assertTrue("first frame $shown", shown[0] > 0.05f)
        // and as far in a frame later as the tween was two frames later
        assertEquals(tween(Motion.MenuOpen, 0f, 1f, 22L), shown[1], 1e-5f)
        assertTrue("$shown", shown[0] < shown[1] && shown[1] < shown[2])
        // at the moment it is asked for it is nothing yet: composed ahead (a held brush's, 50 ms
        // before the hold is reached), no frame shows it before then, at scale nothing (no touch hits it)
        assertEquals(0f, menuLook(true, 0f, Motion.MENU_START_SCALE, 0L).first, 0f)
        for (ahead in listOf(-MENU_AHEAD_MS, -40L, -17L, -1L)) {
            val (a, k) = menuLook(true, 0f, Motion.MENU_START_SCALE, ahead)
            assertEquals(0f, a, 0f)
            assertEquals(Motion.MENU_START_SCALE, k, 0f)
        }
    }

    @Test
    fun aMenuOpensFromAnAskOnlyAFrameOrTwoAway() {
        val now = 10_000L
        // asked for a moment ago (the hold decided, the tap): from then
        assertEquals(now - 12L, menuStart(now - 12L, now))
        assertEquals(now - MENU_ASKED_TRUST_MS, menuStart(now - MENU_ASKED_TRUST_MS, now))
        // composed ahead of a hold still to come: from then
        assertEquals(now + MENU_AHEAD_MS, menuStart(now + MENU_AHEAD_MS, now))
        assertTrue(MENU_AHEAD_MS <= MENU_ASKED_TRUST_MS)
        // none, or one long gone (a row's menu put up again as it scrolls back) or far ahead: from now
        assertEquals(now, menuStart(0L, now))
        assertEquals(now, menuStart(now - MENU_ASKED_TRUST_MS - 1L, now))
        assertEquals(now, menuStart(now + MENU_ASKED_TRUST_MS + 1L, now))
    }

    @Test
    fun theAnimatorScaleSpeedsOrStopsIt() {
        // animations off: at once; slowed down five times: a fifth as far
        assertEquals(1f, menuProgress(0L, 0f), 0f)
        assertEquals(menuProgress(30L), menuProgress(150L, 5f), 1e-6f)
        assertEquals(1f, menuProgress(Motion.MENU_MS.toLong()), 0f)
        assertEquals(0f, menuProgress(-5L), 0f)
    }
}
