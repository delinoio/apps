// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

/** Access only on LaneRuntime's serial worker. No domain rules live in this adapter. */
internal class NativeStore(root: String, private val host: LaneHostServices) : AutoCloseable {
    private var handle = create(root.toByteArray(Charsets.UTF_8))
    fun open(request: String) = execute(0, "", request)
    fun invoke(method: String, request: String) = execute(1, method, request)
    fun release(space: String) = execute(2, "", space)
    fun poll() = execute(3, "", "")
    fun approve(identity: String, merge: Boolean) = execute(4, merge.toString(), identity)
    fun acquire(space: String, owner: String, seconds: Int) = execute(5, space, "$owner\n$seconds")
    fun pending(space: String, owner: String) = execute(6, space, owner)
    fun apply(space: String, response: String) = execute(7, space, response)
    fun confirmEpoch(space: String, epoch: String) = execute(13, space, epoch)
    fun accountSpaces(account: String) = execute(12, "", account)
    fun syncFailure(space: String, reason: String) = execute(11, space, reason)
    fun claim(space: String, owner: String) = execute(8, space, owner)
    fun acknowledge(space: String, proposal: String) = execute(9, space, proposal)
    fun permission(space: String, permission: String) = execute(10, space, permission)
    // Called synchronously by JNI on the same serial worker. Never exposed to JavaScript.
    @androidx.annotation.Keep
    private fun hostCall(operation: Int, space: ByteArray, method: ByteArray, request: ByteArray): ByteArray =
        host.call(operation, space.toString(Charsets.UTF_8), method.toString(Charsets.UTF_8),
            request.toString(Charsets.UTF_8)).toByteArray(Charsets.UTF_8)
    private fun execute(operation: Int, method: String, request: String): String {
        check(handle != 0L)
        return call(handle, operation, method.toByteArray(Charsets.UTF_8),
            request.toByteArray(Charsets.UTF_8)).toString(Charsets.UTF_8)
    }
    fun log(event: String) { if (handle != 0L) logEvent(handle, event.toByteArray(Charsets.UTF_8)) }
    private external fun logEvent(handle: Long, event: ByteArray)
    override fun close() {
        if (handle != 0L) destroy(handle)
        handle = 0L
    }
    private external fun create(root: ByteArray): Long
    private external fun call(handle: Long, operation: Int, method: ByteArray, request: ByteArray): ByteArray
    private external fun destroy(handle: Long)
    companion object { init { System.loadLibrary("lane_android") } }
}
