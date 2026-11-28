package io.github.artcraftandroid.procraft.ui

import androidx.compose.foundation.OverscrollEffect
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.gestures.FlingBehavior
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.ScrollScope
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.LayoutModifierNode
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Velocity
import io.github.artcraftandroid.procraft.core.ScrollMotion
import io.github.artcraftandroid.procraft.core.scrollCoastAt
import io.github.artcraftandroid.procraft.core.scrollCoastVelocity
import io.github.artcraftandroid.procraft.core.scrollDrag
import io.github.artcraftandroid.procraft.core.scrollDragVelocity
import io.github.artcraftandroid.procraft.core.scrollFlingVelocity
import io.github.artcraftandroid.procraft.core.scrollMotion
import io.github.artcraftandroid.procraft.core.scrollMotionAt
import io.github.artcraftandroid.procraft.core.scrollPullDistance
import kotlin.math.abs

@Composable
internal fun rememberScrollFling(): FlingBehavior {
    val density = LocalDensity.current.density
    return remember(density) { ScrollFling(density) }
}

private class ScrollFling(private val density: Float) : FlingBehavior {
    override suspend fun ScrollScope.performFling(initialVelocity: Float): Float {
        val velocity = initialVelocity / density
        var travelled = 0f
        val start = withFrameNanos { it }
        while (true) {
            val seconds = withFrameNanos { (it - start) / 1e9f }
            val sample = scrollCoastAt(velocity, seconds)
            val wanted = (sample.position - travelled) * density
            val consumed = scrollBy(wanted)
            travelled += consumed / density
            if (abs(wanted - consumed) > 0.5f) return scrollCoastVelocity(velocity, travelled) * density
            if (sample.done) return 0f
        }
    }
}

@Composable
internal fun rememberScrollEdges(orientation: Orientation = Orientation.Vertical): OverscrollEffect {
    val density = LocalDensity.current.density
    return remember(orientation, density) { ScrollEdges(orientation, density) }
}

@Composable
internal fun Modifier.kineticVerticalScroll(state: ScrollState): Modifier =
    verticalScroll(state, overscrollEffect = rememberScrollEdges(), flingBehavior = rememberScrollFling())

@Composable
internal fun Modifier.kineticHorizontalScroll(state: ScrollState): Modifier =
    horizontalScroll(
        state,
        overscrollEffect = rememberScrollEdges(Orientation.Horizontal),
        flingBehavior = rememberScrollFling(),
    )

/** Compose lays out and virtualizes the list. Rust calculates the exposed distance at an edge. */
private class ScrollEdges(orientation: Orientation, private val density: Float) : OverscrollEffect {
    private val vertical = orientation == Orientation.Vertical
    private var displacement by mutableFloatStateOf(0f)
    private var viewport = 1f
    private var generation = 0L
    private var returning = false
    private var activeFling: Pair<Float, Long>? = null
    private var caught: Pair<Float, Long>? = null

    private fun along(offset: Offset) = if (vertical) offset.y else offset.x
    private fun along(speed: Velocity) = if (vertical) speed.y else speed.x
    private fun offset(amount: Float) = if (vertical) Offset(0f, amount) else Offset(amount, 0f)
    private fun reboundVelocity(amount: Float) = if (vertical) Velocity(0f, amount) else Velocity(amount, 0f)

    override val node: DelegatableNode = object : Modifier.Node(), LayoutModifierNode {
        override fun MeasureScope.measure(measurable: Measurable, constraints: Constraints): MeasureResult {
            val child = measurable.measure(constraints)
            viewport = (if (vertical) child.height else child.width) / density
            return layout(child.width, child.height) {
                child.placeWithLayer(0, 0) {
                    if (vertical) translationY = displacement * density else translationX = displacement * density
                }
            }
        }

        override fun onDetach() {
            generation++
            returning = false
            displacement = 0f
            activeFling = null
            caught = null
        }
    }

    override val isInProgress: Boolean get() = displacement != 0f || returning

    override fun applyToScroll(
        delta: Offset,
        source: NestedScrollSource,
        performScroll: (Offset) -> Offset,
    ): Offset {
        if (source != NestedScrollSource.UserInput) return performScroll(delta)
        activeFling?.let { (speed, start) ->
            val now = System.nanoTime()
            caught = scrollCoastAt(speed, (now - start) / 1e9f).velocity to now
            activeFling = null
        }
        generation++
        returning = false
        val input = along(delta) / density
        var unwound = 0f
        if (input * displacement < 0f) {
            val pull = scrollPullDistance(displacement, viewport)
            unwound = if (abs(input) < abs(pull)) input else -pull
            displacement = if (unwound == -pull) 0f else scrollDrag(displacement, unwound, 0f, 0f, viewport)
        }
        val released = offset(unwound * density)
        val available = delta - released
        val consumed = performScroll(available)
        val remaining = along(available - consumed) / density
        if (remaining == 0f) return released + consumed
        displacement = scrollDrag(displacement, remaining, 0f, 0f, viewport)
        return released + consumed + offset(remaining * density)
    }

    override suspend fun applyToFling(velocity: Velocity, performFling: suspend (Velocity) -> Velocity) {
        val gesture = ++generation
        val held = displacement
        val now = System.nanoTime()
        val incoming = along(velocity) / density
        val launch = caught?.let { (previous, touched) ->
            scrollFlingVelocity(incoming, previous, (now - touched) / 1e9f)
        } ?: incoming
        caught = null
        activeFling = null
        val available = if (held == 0f) reboundVelocity(launch * density) else {
            val speed = scrollDragVelocity(held, launch, 0f, 0f, viewport)
            val rebound = returnToEdge(held, speed, gesture) ?: return
            reboundVelocity(rebound * density)
        }
        activeFling = along(available) / density to System.nanoTime()
        val remaining = along(available - performFling(available)) / density
        if (gesture == generation) activeFling = null
        if (remaining != 0f && gesture == generation) returnToEdge(0f, remaining, gesture)
    }

    private suspend fun returnToEdge(from: Float, velocity: Float, gesture: Long): Float? {
        val motion = scrollMotion(from, velocity, 0f, 0f, 0f)
        returning = true
        try {
            val start = withFrameNanos { it }
            while (gesture == generation) {
                val time = withFrameNanos { (it - start) / 1e9f }
                if (gesture != generation) return null
                if (motion is ScrollMotion.Reenter && time >= motion.hitTime) {
                    displacement = 0f
                    return motion.hitVelocity
                }
                val sample = scrollMotionAt(motion, time)
                displacement = sample.position
                if (sample.done) return 0f
            }
            return null
        } finally {
            if (gesture == generation) returning = false
        }
    }
}
