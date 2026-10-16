package se.eyevinn.tallycam.media

import android.media.Image

/** The metering of the auto ISO loop, from a YUV_420_888 frame; no allocation. */
internal object Luma {
    /** Mid grey in gamma-coded luma (0.18 through the BT.709 curve). */
    const val TARGET = 0.41

    /** About 80 samples across. */
    private const val COLUMNS = 80

    /**
     * Mean luma, 0..1: the whole frame, or with a metering point (tap to
     * focus and expose) a fifth of the frame around it at three quarters'
     * weight.
     */
    fun meteredMean(image: Image, point: Pair<Float, Float>?): Double {
        val w = image.width
        val h = image.height
        val whole = mean(image, 0, 0, w, h)
        point ?: return whole
        val rw = w / 5
        val rh = h / 5
        val left = ((point.first * w).toInt() - rw / 2).coerceIn(0, w - rw)
        val top = ((point.second * h).toInt() - rh / 2).coerceIn(0, h - rh)
        return 0.75 * mean(image, left, top, rw, rh) + 0.25 * whole
    }

    private fun mean(image: Image, left: Int, top: Int, w: Int, h: Int): Double {
        val plane = image.planes[0]
        val buffer = plane.buffer
        val step = maxOf(1, image.width / COLUMNS)
        var sum = 0L
        var n = 0
        var row = top
        while (row < top + h) {
            val base = row * plane.rowStride
            var col = left
            while (col < left + w) {
                sum += buffer.get(base + col * plane.pixelStride).toInt() and 0xFF
                n++
                col += step
            }
            row += step
        }
        return if (n == 0) TARGET else sum / (255.0 * n)
    }
}

/**
 * The grid the core's image statistics read (as iOS StatsSampler): every
 * `step`-th pixel, `step` even so each sample sits on a chroma site, about
 * 160 columns. The arrays are reused; the caller keeps the grid busy until
 * the core has read it.
 */
internal class StatsGrid {
    var y = ByteArray(0)
        private set
    var cbcr = ByteArray(0)
        private set
    var width = 0
        private set
    var height = 0
        private set

    fun sample(image: Image): Boolean {
        val w = image.width
        val h = image.height
        if (w < 2 || h < 2 || image.planes.size < 3) return false
        val step = maxOf(2, (w / GRID_WIDTH) and 1.inv())
        val columns = w / step
        val rows = h / step
        if (y.size != columns * rows) {
            y = ByteArray(columns * rows)
            cbcr = ByteArray(2 * columns * rows)
        }
        width = columns
        height = rows
        val luma = image.planes[0]
        val u = image.planes[1]
        val v = image.planes[2]
        val yBuf = luma.buffer
        val uBuf = u.buffer
        val vBuf = v.buffer
        var i = 0
        for (row in 0 until rows) {
            val yBase = row * step * luma.rowStride
            val cRow = row * step / 2
            val uBase = cRow * u.rowStride
            val vBase = cRow * v.rowStride
            for (column in 0 until columns) {
                val x = column * step
                y[i] = yBuf.get(yBase + x * luma.pixelStride)
                cbcr[2 * i] = uBuf.get(uBase + (x / 2) * u.pixelStride)
                cbcr[2 * i + 1] = vBuf.get(vBase + (x / 2) * v.pixelStride)
                i++
            }
        }
        return true
    }

    companion object {
        const val GRID_WIDTH = 160
    }
}
