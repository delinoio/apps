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
import java.util.concurrent.atomic.AtomicBoolean

/** Render authorized PNG bytes and bundled artwork without a shared disk/URL cache. */
internal class LaneImageFetcher : LynxImageFetcher, AutoCloseable {
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "LaneImages").apply { isDaemon = true } }
    private val main = Handler(Looper.getMainLooper())
    private val requests = Collections.synchronizedMap(IdentityHashMap<ImageRequestInfo, AtomicBoolean>())
    override fun loadImage(request: ImageRequestInfo, listener: ImageLoadListener,
        animation: AnimationListener?, context: Context) {
        val canceled = AtomicBoolean(false)
        requests.put(request, canceled)?.set(true)
        listener.onRequestSubmit(request)
        worker.execute {
            val bitmap = try {
                val prefix = "data:image/png;base64,"
                require(request.url.startsWith(prefix))
                val bytes = Base64.getDecoder().decode(request.url.substring(prefix.length))
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            } catch (_: Exception) { null }
            main.post {
                if (canceled.get()) { bitmap?.recycle(); return@post }
                requests.remove(request)
                if (bitmap == null) listener.onFailure(1, IllegalArgumentException("Lane image is unavailable"))
                else listener.onSuccess(ImageContent(bitmap), request, ImageInfo(bitmap.width, bitmap.height, false))
            }
        }
    }
    override fun close() {
        synchronized(requests) { requests.values.forEach { it.set(true) }; requests.clear() }
        worker.shutdownNow()
    }
    override fun releaseImage(request: ImageRequestInfo) { requests.remove(request)?.set(true) }
}
