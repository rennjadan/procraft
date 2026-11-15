package io.github.artcraftandroid.procraft.canvas

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeyNamesTest {
    @Test
    fun keysGoByTheNamesTheSharedShortcutsKnowAndBack() {
        val named = mapOf(
            KeyEvent.KEYCODE_Z to "z",
            KeyEvent.KEYCODE_0 to "0",
            KeyEvent.KEYCODE_LEFT_BRACKET to "[",
            KeyEvent.KEYCODE_SPACE to "space",
            KeyEvent.KEYCODE_DEL to "backspace",
            KeyEvent.KEYCODE_FORWARD_DEL to "delete",
            KeyEvent.KEYCODE_ESCAPE to "escape",
            KeyEvent.KEYCODE_PLUS to "plus",
            KeyEvent.KEYCODE_F1 to "f1",
            KeyEvent.KEYCODE_F12 to "f12",
            KeyEvent.KEYCODE_DPAD_UP to "up",
        )
        for ((code, name) in named) {
            assertEquals(name, keyName(code))
            assertEquals(code, keyCodeNamed(name))
        }
        assertNull(keyName(KeyEvent.KEYCODE_TAB))
        assertNull(keyCodeNamed("tab"))
    }

    @Test
    fun ctrlAndMetaAreBothTheCommandKey() {
        val ctrl = KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        val meta = KeyEvent.META_META_ON or KeyEvent.META_META_RIGHT_ON
        val shift = KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON
        val alt = KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON
        // undo, redo and the brush's size, as keyboards and tablets' express keys send them
        assertEquals(KeyPress("z", true, false, false), keyPress(KeyEvent.KEYCODE_Z, ctrl))
        assertEquals(KeyPress("z", true, false, false), keyPress(KeyEvent.KEYCODE_Z, meta))
        assertEquals(KeyPress("z", true, true, false), keyPress(KeyEvent.KEYCODE_Z, ctrl or shift))
        assertEquals(KeyPress("y", true, false, false), keyPress(KeyEvent.KEYCODE_Y, ctrl))
        assertEquals(KeyPress("[", false, false, false), keyPress(KeyEvent.KEYCODE_LEFT_BRACKET, 0))
        assertEquals(KeyPress("]", false, true, false), keyPress(KeyEvent.KEYCODE_RIGHT_BRACKET, shift))
        assertEquals(KeyPress("e", false, false, false), keyPress(KeyEvent.KEYCODE_E, KeyEvent.META_NUM_LOCK_ON))
        assertEquals(KeyPress("0", true, false, true), keyPress(KeyEvent.KEYCODE_0, ctrl or alt))
        // a key the shortcuts don't name is no press of theirs, whatever is held
        assertNull(keyPress(KeyEvent.KEYCODE_TAB, ctrl))
        assertNull(keyPress(KeyEvent.KEYCODE_CTRL_LEFT, ctrl))
    }
}
