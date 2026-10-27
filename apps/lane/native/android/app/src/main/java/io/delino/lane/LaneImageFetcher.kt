// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.Context
import android.graphics.BitmapFactory
import android.os.Handler
import android.os.Looper
import com.lynx.tasm.image.ImageContent
import com.lynx.tasm.image.model.AnimationListener
import com.lynx.tasm.image.model.ImageInfo
import com.lynx.tasm.image.model.ImageLoadListener
import com.lynx.tasm.image.model.ImageRequestInfo
import com.lynx.tasm.image.model.LynxImageFetcher
import java.util.Base64
import java.util.Collections
import java.util.IdentityHashMap
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean

/** Render HTTPS Markdown images and host-authorized inline PNGs without a shared disk/URL cache. */
internal class LaneImageFetcher : LynxImageFetcher, AutoCloseable {
    private val worker = Executors.newFixedThreadPool(4) { Thread(it, "LaneImages").apply { isDaemon = true } }
    private val main = Handler(Looper.getMainLooper())
    private val requests = Collections.synchronizedMap(IdentityHashMap<ImageRequestInfo, LaneHttpsImage.Cancellation>())
    private val closed = AtomicBoolean(false)
    override fun loadImage(request: ImageRequestInfo, listener: ImageLoadListener,
        animation: AnimationListener?, context: Context) {
        if (closed.get()) return
        val cancellation = LaneHttpsImage.Cancellation()
        requests.put(request, cancellation)?.cancel()
        listener.onRequestSubmit(request)
        try {
            worker.execute {
                val bitmap = try {
                    val prefix = "data:image/png;base64,"
                    if (request.url.startsWith(prefix)) {
                        val bytes = Base64.getDecoder().decode(request.url.substring(prefix.length))
                        BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
                    } else {
                        val bytes = LaneHttpsImage.load(request.url, cancellation)
                        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
                        if (bounds.outWidth <= 0 || bounds.outHeight <= 0 ||
                            bounds.outWidth.toLong() * bounds.outHeight > 16_000_000L) null
                        else BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
                    }
                } catch (_: Exception) { null }
                main.post {
                    synchronized(requests) {
                        if (requests[request] !== cancellation || cancellation.canceled.get()) {
                            bitmap?.recycle()
                            return@post
                        }
                        try {
                            if (bitmap == null) listener.onFailure(1, IllegalArgumentException("Lane image is unavailable"))
                            else listener.onSuccess(ImageContent(bitmap), request, ImageInfo(bitmap.width, bitmap.height, false))
                        } finally {
                            if (requests[request] === cancellation) requests.remove(request)
                        }
                    }
                }
            }
        } catch (_: RejectedExecutionException) {
            requests.remove(request)?.cancel()
        }
    }
    override fun close() {
        closed.set(true)
        synchronized(requests) { requests.values.forEach { it.cancel() }; requests.clear() }
        worker.shutdownNow()
    }
    override fun releaseImage(request: ImageRequestInfo) { requests.remove(request)?.cancel() }
}
