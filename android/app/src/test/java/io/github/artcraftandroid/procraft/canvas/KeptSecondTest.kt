package io.github.artcraftandroid.procraft.canvas

import org.junit.Assert.assertEquals
import org.junit.Test

class KeptSecondTest {
    @Test
    fun everySecondColourComesBackAsItWasKept() {
        // each channel's every level, and colours across them
        for (v in 0..255) {
            for (rgb in listOf(v shl 16, v shl 8, v, (v shl 16) or ((255 - v) shl 8) or (v * 7 % 256))) {
                assertEquals(rgb, secondRgbOf(keptOfSecond(rgb)))
            }
        }
        assertEquals(listOf(1f, 1f, 1f), keptOfSecond(0xFFFFFF))
        // only the colour's 24 bits count
        assertEquals(keptOfSecond(0x123456), keptOfSecond(0x7F123456))
    }

    @Test
    fun whatIsntAColourIsWhiteAsAFirstStartHasIt() {
        assertEquals(0xFFFFFF, secondRgbOf(emptyList()))
        assertEquals(0xFFFFFF, secondRgbOf(listOf(0.1f, 0.2f)))
        assertEquals(0xFFFFFF, secondRgbOf(listOf(0.1f, Float.NaN, 0.3f)))
        // out of range is clamped
        assertEquals(0xFF0080, secondRgbOf(listOf(2f, -1f, 0.5f)))
    }
}
