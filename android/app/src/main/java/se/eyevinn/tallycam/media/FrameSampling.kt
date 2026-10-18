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

/**
 * Focus peaking as an overlay (docs/SHADING.md, D10), as iOS
 * PeakingProcessor: the luma plane downscaled to 320 wide (camera thread),
 * a 3x3 Sobel, and the strongest 7 % of edges above a floor as opaque
 * pixels of an ALPHA_8 mask (analysis thread; a quarter of ARGB's bytes,
 * drawn in the paint's colour). Two bitmaps alternate so the one on screen
 * is never written. Nothing is allocated per frame.
 */
internal class Peaking {
    private var small = ByteArray(0)
    private var gradient = IntArray(0)
    private val histogram = IntArray(256)
    /** The mask's bytes, rows [android.graphics.Bitmap.getRowBytes] apart. */
    private var pixels = java.nio.ByteBuffer.allocate(0)
    private val bitmaps = arrayOfNulls<android.graphics.Bitmap>(2)
    private var next = 0
    var width = 0
        private set
    var height = 0
        private set

    /** Camera thread: the frame's luma, downscaled. */
    fun sample(image: Image): Boolean {
        val w = image.width
        val h = image.height
        if (w < 3 || h < 3) return false
        val outW = minOf(WIDTH, w)
        val outH = maxOf(3, h * outW / w)
        if (small.size != outW * outH) small = ByteArray(outW * outH)
        width = outW
        height = outH
        val plane = image.planes[0]
        val buffer = plane.buffer
        var i = 0
        for (y in 0 until outH) {
            val base = (y * h / outH) * plane.rowStride
            for (x in 0 until outW) {
                small[i++] = buffer.get(base + (x * w / outW) * plane.pixelStride)
            }
        }
        return true
    }

    /** Analysis thread: the mask of the sampled frame, white where it is sharp. */
    fun mask(): android.graphics.Bitmap {
        val w = width
        val h = height
        if (gradient.size != w * h) gradient = IntArray(w * h)
        histogram.fill(0)
        val luma = small
        for (y in 1 until h - 1) {
            val above = (y - 1) * w
            val row = y * w
            val below = (y + 1) * w
            for (x in 1 until w - 1) {
                val a = luma[above + x - 1].toInt() and 0xFF
                val b = luma[above + x].toInt() and 0xFF
                val c = luma[above + x + 1].toInt() and 0xFF
                val d = luma[row + x - 1].toInt() and 0xFF
                val f = luma[row + x + 1].toInt() and 0xFF
                val g = luma[below + x - 1].toInt() and 0xFF
                val hh = luma[below + x].toInt() and 0xFF
                val ii = luma[below + x + 1].toInt() and 0xFF
                val gx = (c + 2 * f + ii) - (a + 2 * d + g)
                val gy = (g + 2 * hh + ii) - (a + 2 * b + c)
                val magnitude = (if (gx < 0) -gx else gx) + (if (gy < 0) -gy else gy)
                gradient[row + x] = magnitude
                histogram[minOf(magnitude shr 3, 255)]++
            }
        }
        val threshold = maxOf(MINIMUM_GRADIENT, percentile((w - 2) * (h - 2)))
        val slot = next
        next = 1 - next
        var bitmap = bitmaps[slot]
        if (bitmap == null || bitmap.width != w || bitmap.height != h) {
            bitmap = android.graphics.Bitmap.createBitmap(w, h, android.graphics.Bitmap.Config.ALPHA_8)
            bitmaps[slot] = bitmap
        }
        val stride = bitmap!!.rowBytes
        if (pixels.capacity() != stride * h) pixels = java.nio.ByteBuffer.allocate(stride * h)
        val mask = pixels.array()
        for (y in 0 until h) {
            var k = y * w
            var o = y * stride
            for (x in 0 until w) mask[o++] = if (gradient[k++] >= threshold) OPAQUE else 0
        }
        pixels.rewind()
        bitmap.copyPixelsFromBuffer(pixels)
        return bitmap
    }

    /** The gradient above which EDGE_SHARE of the pixels lie (bins of 8). */
    private fun percentile(total: Int): Int {
        val wanted = (total * EDGE_SHARE).toInt()
        var count = 0
        for (bin in 255 downTo 0) {
            count += histogram[bin]
            if (count >= wanted) return bin shl 3
        }
        return 0
    }

    companion object {
        private const val WIDTH = 320
        private const val OPAQUE = 0xFF.toByte()
        private const val EDGE_SHARE = 0.07
        /** A soft picture shows no edges rather than its noise. */
        private const val MINIMUM_GRADIENT = 150
    }
}
