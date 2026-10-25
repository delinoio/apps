// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.Context
import com.lynx.jsbridge.LynxMethod
import com.lynx.jsbridge.LynxModule
import com.lynx.react.bridge.Callback

class LaneStoreModule(context: Context, param: Any) : LynxModule(context, param) {
    private val session = param as LaneSession
    @LynxMethod fun open(requestJson: String, callback: Callback) = session.runtime.open(session, requestJson, callback)
    @LynxMethod fun invoke(method: String, requestJson: String, callback: Callback) =
        session.runtime.invoke(session, method, requestJson, callback)
    @LynxMethod fun close(spaceId: String, callback: Callback) = session.runtime.close(session, spaceId, callback)
}
