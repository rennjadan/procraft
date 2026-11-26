package io.github.artcraftandroid.procraft.canvas

/**
 * Whether Paste takes the picture on the system's clipboard, put there at [systemAt] (the wall
 * clock, as the system keeps it), over the copy made here ([ownCopy], made at [ownAt]): only one
 * put there after that copy, and after the copy itself went there ([ownPutAt], the time the system
 * gave it then, if it took it), and not the copy's own picture ([ours]). A system that puts a copy
 * of its own in place of ours as it takes it (a clipboard of its own) still pastes ours.
 */
internal fun pastesSystemPicture(ours: Boolean, systemAt: Long, ownCopy: Boolean, ownAt: Long, ownPutAt: Long?): Boolean =
    !ours && (!ownCopy || systemAt > maxOf(ownAt, ownPutAt ?: Long.MIN_VALUE))

/**
 * The extra that marks a clip sensitive: the system shows no preview of it, and keyboards leave it
 * out of their clipboards and suggestions. Android 13 named it `ClipDescription.EXTRA_IS_SENSITIVE`;
 * earlier systems and keyboards read the same key, so it is written out here for every version.
 */
internal const val CLIP_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"

/** The extras a picture cut or copied here goes on the system's clipboard with (see [CLIP_IS_SENSITIVE]). */
internal val PICTURE_CLIP_EXTRAS: Map<String, Boolean> = mapOf(CLIP_IS_SENSITIVE to true)

/**
 * [clip] marked as a picture cut or copied here: kept out of the keyboard's clipboard, which showed
 * each cut there to be deleted by hand (#403). Paste takes it as before, here and in other apps.
 */
internal fun markedAsPictureCopy(clip: android.content.ClipData): android.content.ClipData {
    clip.description.extras = android.os.PersistableBundle().apply {
        PICTURE_CLIP_EXTRAS.forEach { (key, on) -> putBoolean(key, on) }
    }
    return clip
}
