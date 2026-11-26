package io.github.artcraftandroid.procraft.canvas

/**
 * The second colour (0xRRGGBB) from the red, green and blue (0..1) the core keeps it as; white for
 * anything that isn't three numbers, as a first start has it.
 */
internal fun secondRgbOf(kept: List<Float>): Int {
    if (kept.size != 3 || kept.any { !it.isFinite() }) return 0xFFFFFF
    val (r, g, b) = kept.map { (it.coerceIn(0f, 1f) * 255f + 0.5f).toInt().coerceIn(0, 255) }
    return (r shl 16) or (g shl 8) or b
}

/** The second colour (0xRRGGBB) as the core keeps it: red, green and blue, 0..1. */
internal fun keptOfSecond(rgb: Int): List<Float> =
    listOf(((rgb shr 16) and 0xFF) / 255f, ((rgb shr 8) and 0xFF) / 255f, (rgb and 0xFF) / 255f)
