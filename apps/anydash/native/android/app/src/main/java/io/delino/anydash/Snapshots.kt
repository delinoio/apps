// SPDX-License-Identifier: Apache-2.0
// Read-only decoding of the anydash.v1 messages the Android widget displays:
// NativeWidgetSnapshot (written by the app) and the WidgetData summary from
// QueryWidgetData. Field numbers follow protos/anydash/v1/anydash.proto and
// match the shared Swift decoder; tests decode the TypeScript-generated fixtures.
package io.delino.anydash

import java.math.RoundingMode
import java.text.NumberFormat
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale
import java.util.TimeZone

enum class SharingState(val wire: Int) {
    UNSPECIFIED(0), PRIVATE(1), SHARED(2), ENDED(3);
    companion object { fun of(value: Long) = entries.firstOrNull { it.wire.toLong() == value } ?: UNSPECIFIED }
}

enum class MetricState(val wire: Int) {
    UNSPECIFIED(0), VALUE(1), NOT_PROVIDED(2), FAILED(3), AUTH_EXPIRED(4), PARTIAL(5), CANNOT_BE_CALCULATED(6);
    companion object { fun of(value: Long) = entries.firstOrNull { it.wire.toLong() == value } ?: UNSPECIFIED }
}

sealed interface MetricContent {
    data object None : MetricContent
    data class Percent(val value: Double) : MetricContent
    data class Decimal(val value: String) : MetricContent
    data class Integer(val value: Long) : MetricContent
    data class Money(val currency: String, val amount: String) : MetricContent
    data class Time(val epochMillis: Long) : MetricContent
    data class Duration(val seconds: Double) : MetricContent
    data class Text(val value: String) : MetricContent
    data class Flag(val value: Boolean) : MetricContent
}

data class MetricValue(val state: MetricState = MetricState.UNSPECIFIED, val content: MetricContent = MetricContent.None)

data class WidgetSummary(
    val title: String = "",
    val primaryLabel: String = "",
    val primaryValue: MetricValue = MetricValue(),
    val secondaryLabel: String = "",
    val secondaryValue: MetricValue = MetricValue(),
    val dataUpdatedAt: Long? = null,
)

data class NativeWidgetSnapshot(
    val dashboardId: String = "",
    val widgetId: String = "",
    val spaceId: String = "",
    val sharingState: SharingState = SharingState.UNSPECIFIED,
    val summary: WidgetSummary? = null,
    val refreshedAt: Long? = null,
) {
    /** anydash://spaces/<space>/dashboards/<dashboard>?widget=<widget> */
    val deepLink: String get() = "anydash://spaces/$spaceId/dashboards/$dashboardId?widget=$widgetId"
    val ended: Boolean get() = sharingState == SharingState.ENDED || summary == null
}

class DecodeException(message: String) : Exception(message)

/** Minimal Protobuf wire reader; unknown fields are skipped. */
private class WireReader(private val data: ByteArray) {
    private var offset = 0
    val atEnd get() = offset >= data.size
    fun varint(): Long {
        var result = 0L
        var shift = 0
        while (true) {
            if (offset >= data.size || shift >= 64) throw DecodeException("truncated varint")
            val byte = data[offset++].toInt() and 0xff
            result = result or ((byte and 0x7f).toLong() shl shift)
            if (byte and 0x80 == 0) return result
            shift += 7
        }
    }
    fun tag(): Pair<Int, Int> = varint().let { (it ushr 3).toInt() to (it and 7).toInt() }
    fun bytes(): ByteArray {
        val length = varint()
        if (length < 0 || offset + length > data.size) throw DecodeException("truncated bytes")
        return data.copyOfRange(offset, offset + length.toInt()).also { offset += length.toInt() }
    }
    fun string() = bytes().toString(Charsets.UTF_8)
    fun fixed64(): Long {
        if (offset + 8 > data.size) throw DecodeException("truncated fixed64")
        var value = 0L
        for (index in 0 until 8) value = value or ((data[offset + index].toLong() and 0xff) shl (8 * index))
        offset += 8
        return value
    }
    fun skip(wire: Int) {
        when (wire) {
            0 -> varint()
            1 -> fixed64()
            2 -> bytes()
            5 -> if (offset + 4 <= data.size) offset += 4 else throw DecodeException("truncated fixed32")
            else -> throw DecodeException("unsupported wire type $wire")
        }
    }
}

object SnapshotCodec {
    private fun timestamp(bytes: ByteArray): Long {
        val reader = WireReader(bytes)
        var seconds = 0L
        var nanos = 0L
        while (!reader.atEnd) {
            val (field, wire) = reader.tag()
            when {
                field == 1 && wire == 0 -> seconds = reader.varint()
                field == 2 && wire == 0 -> nanos = reader.varint().toInt().toLong()
                else -> reader.skip(wire)
            }
        }
        return seconds * 1000 + nanos / 1_000_000
    }

    private fun metric(bytes: ByteArray): MetricValue {
        val reader = WireReader(bytes)
        var state = MetricState.UNSPECIFIED
        var content: MetricContent = MetricContent.None
        while (!reader.atEnd) {
            val (field, wire) = reader.tag()
            when {
                field == 1 && wire == 0 -> state = MetricState.of(reader.varint())
                field == 2 && wire == 1 -> content = MetricContent.Percent(Double.fromBits(reader.fixed64()))
                field == 3 && wire == 2 -> content = MetricContent.Decimal(reader.string())
                field == 4 && wire == 0 -> content = MetricContent.Integer(reader.varint())
                field == 5 && wire == 2 -> {
                    val money = WireReader(reader.bytes())
                    var currency = ""
                    var amount = ""
                    while (!money.atEnd) {
                        val (moneyField, moneyWire) = money.tag()
                        when {
                            moneyField == 1 && moneyWire == 2 -> currency = money.string()
                            moneyField == 2 && moneyWire == 2 -> amount = money.string()
                            else -> money.skip(moneyWire)
                        }
                    }
                    content = MetricContent.Money(currency, amount)
                }
                field == 6 && wire == 2 -> content = MetricContent.Time(timestamp(reader.bytes()))
                field == 7 && wire == 2 -> content = MetricContent.Duration(timestamp(reader.bytes()) / 1000.0)
                field == 8 && wire == 2 -> content = MetricContent.Text(reader.string())
                field == 9 && wire == 0 -> content = MetricContent.Flag(reader.varint() != 0L)
                else -> reader.skip(wire)
            }
        }
        return MetricValue(state, content)
    }

    private fun summary(bytes: ByteArray): WidgetSummary {
        val reader = WireReader(bytes)
        var summary = WidgetSummary()
        while (!reader.atEnd) {
            val (field, wire) = reader.tag()
            summary = when {
                field == 1 && wire == 2 -> summary.copy(title = reader.string())
                field == 2 && wire == 2 -> summary.copy(primaryLabel = reader.string())
                field == 3 && wire == 2 -> summary.copy(primaryValue = metric(reader.bytes()))
                field == 4 && wire == 2 -> summary.copy(secondaryLabel = reader.string())
                field == 5 && wire == 2 -> summary.copy(secondaryValue = metric(reader.bytes()))
                field == 6 && wire == 2 -> summary.copy(dataUpdatedAt = timestamp(reader.bytes()))
                else -> { reader.skip(wire); summary }
            }
        }
        return summary
    }

    fun snapshot(bytes: ByteArray): NativeWidgetSnapshot {
        val reader = WireReader(bytes)
        var snapshot = NativeWidgetSnapshot()
        while (!reader.atEnd) {
            val (field, wire) = reader.tag()
            snapshot = when {
                field == 1 && wire == 2 -> snapshot.copy(dashboardId = reader.string())
                field == 2 && wire == 2 -> snapshot.copy(widgetId = reader.string())
                field == 3 && wire == 2 -> snapshot.copy(spaceId = reader.string())
                field == 4 && wire == 0 -> snapshot.copy(sharingState = SharingState.of(reader.varint()))
                field == 5 && wire == 2 -> snapshot.copy(summary = summary(reader.bytes()))
                field == 6 && wire == 2 -> snapshot.copy(refreshedAt = timestamp(reader.bytes()))
                else -> { reader.skip(wire); snapshot }
            }
        }
        // ENDED snapshots never display data, even if a summary was present.
        return if (snapshot.sharingState == SharingState.ENDED) snapshot.copy(summary = null) else snapshot
    }

    /** ListNativeWidgetSnapshotsResponse: field 1 repeats NativeWidgetSnapshot. */
    fun snapshotList(bytes: ByteArray): List<NativeWidgetSnapshot> {
        val reader = WireReader(bytes)
        val snapshots = mutableListOf<NativeWidgetSnapshot>()
        while (!reader.atEnd) {
            val (field, wire) = reader.tag()
            if (field == 1 && wire == 2) snapshots += snapshot(reader.bytes()) else reader.skip(wire)
        }
        return snapshots
    }

    /** The WidgetData sharing state and summary, with the summary's wire bytes for re-encoding. */
    data class WidgetDataSummary(val state: SharingState, val summary: WidgetSummary?, val summaryBytes: ByteArray?)

    /** QueryWidgetDataResponse → the sharing state and summary of WidgetData (field 1). */
    fun widgetDataSummary(bytes: ByteArray): WidgetDataSummary {
        val outer = WireReader(bytes)
        var state = SharingState.UNSPECIFIED
        var summary: WidgetSummary? = null
        var summaryBytes: ByteArray? = null
        while (!outer.atEnd) {
            val (field, wire) = outer.tag()
            if (field != 1 || wire != 2) { outer.skip(wire); continue }
            val reader = WireReader(outer.bytes())
            while (!reader.atEnd) {
                val (inner, innerWire) = reader.tag()
                when {
                    inner == 3 && innerWire == 0 -> state = SharingState.of(reader.varint())
                    inner == 9 && innerWire == 2 -> {
                        summaryBytes = reader.bytes()
                        summary = summary(summaryBytes)
                    }
                    else -> reader.skip(innerWire)
                }
            }
        }
        return if (state == SharingState.ENDED) WidgetDataSummary(state, null, null)
        else WidgetDataSummary(state, summary, summaryBytes)
    }

    private fun varint(value: Long, out: MutableList<Byte>) {
        var rest = value
        do {
            var byte = (rest and 0x7f).toInt()
            rest = rest ushr 7
            if (rest != 0L) byte = byte or 0x80
            out += byte.toByte()
        } while (rest != 0L)
    }

    private fun bytes(field: Int, value: ByteArray, out: MutableList<Byte>) {
        if (value.isEmpty()) return
        out += (field shl 3 or 2).toByte()
        varint(value.size.toLong(), out)
        out += value.toList()
    }

    private fun string(field: Int, value: String, out: MutableList<Byte>) = bytes(field, value.toByteArray(Charsets.UTF_8), out)

    /**
     * NativeWidgetSnapshot after a widget-side refresh. The summary is copied
     * from the QueryWidgetData response bytes, so no field is re-encoded.
     */
    fun encodeSnapshot(snapshot: NativeWidgetSnapshot, summaryBytes: ByteArray?, refreshedAt: Long): ByteArray {
        val out = mutableListOf<Byte>()
        string(1, snapshot.dashboardId, out)
        string(2, snapshot.widgetId, out)
        string(3, snapshot.spaceId, out)
        if (snapshot.sharingState != SharingState.UNSPECIFIED) {
            out += (4 shl 3).toByte()
            varint(snapshot.sharingState.wire.toLong(), out)
        }
        if (snapshot.sharingState != SharingState.ENDED && summaryBytes != null) {
            out += (5 shl 3 or 2).toByte()
            varint(summaryBytes.size.toLong(), out)
            out += summaryBytes.toList()
        }
        val time = mutableListOf<Byte>()
        val seconds = Math.floorDiv(refreshedAt, 1000L)
        val nanos = Math.floorMod(refreshedAt, 1000L) * 1_000_000
        if (seconds != 0L) { time += (1 shl 3).toByte(); varint(seconds, time) }
        if (nanos != 0L) { time += (2 shl 3).toByte(); varint(nanos, time) }
        bytes(6, time.toByteArray(), out)
        return out.toByteArray()
    }

    /** QueryWidgetDataRequest(dashboard_id, widget_id, time_zone). */
    fun queryWidgetData(dashboardId: String, widgetId: String, timeZone: String): ByteArray {
        val out = mutableListOf<Byte>()
        string(1, dashboardId, out)
        string(2, widgetId, out)
        string(3, timeZone, out)
        return out.toByteArray()
    }
}

/** English display strings matching the design record's native widget and the Swift widgets. */
object WidgetText {
    const val SHARING_ENDED = "Sharing ended"
    const val SHARING_ENDED_DETAIL = "Open AnyDash to choose another widget."
    const val NOT_PROVIDED = "Not provided"
    const val EMPTY = "Choose a widget in AnyDash."

    private fun sameDay(a: Long, b: Long, zone: TimeZone): Boolean {
        val first = Calendar.getInstance(zone).apply { timeInMillis = a }
        val second = Calendar.getInstance(zone).apply { timeInMillis = b }
        return first.get(Calendar.YEAR) == second.get(Calendar.YEAR) &&
            first.get(Calendar.DAY_OF_YEAR) == second.get(Calendar.DAY_OF_YEAR)
    }

    fun time(at: Long, now: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): String {
        val format = SimpleDateFormat(if (sameDay(at, now, zone)) "HH:mm" else "MMM d, HH:mm", Locale.US)
        format.timeZone = zone
        return format.format(Date(at))
    }

    fun updated(at: Long, now: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()) =
        "Updated ${time(at, now, zone)}"

    fun value(metric: MetricValue, now: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): String {
        if (metric.state == MetricState.NOT_PROVIDED) return NOT_PROVIDED
        return when (val content = metric.content) {
            is MetricContent.Percent -> "${Math.round(content.value)}%"
            is MetricContent.Decimal -> content.value
            is MetricContent.Integer -> NumberFormat.getIntegerInstance(Locale.US).format(content.value)
            is MetricContent.Money -> {
                val text = content.amount.toBigDecimalOrNull()?.setScale(2, RoundingMode.HALF_UP)?.toPlainString()
                    ?: content.amount
                when {
                    content.currency == "USD" -> "$$text"
                    content.currency.isEmpty() -> text
                    else -> "$text ${content.currency}"
                }
            }
            is MetricContent.Time -> time(content.epochMillis, now, zone)
            is MetricContent.Duration ->
                if (content.seconds >= 86400) "${(content.seconds / 86400).toInt()} days" else "${(content.seconds / 3600).toInt()} hours"
            is MetricContent.Text -> content.value
            is MetricContent.Flag -> if (content.value) "On" else "Off"
            MetricContent.None -> ""
        }
    }

    /** "left · Resets 14:20" from the summary labels. */
    fun supporting(summary: WidgetSummary, now: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): String {
        val secondary = listOf(summary.secondaryLabel, value(summary.secondaryValue, now, zone)).filter { it.isNotEmpty() }.joinToString(" ")
        return listOf(summary.primaryLabel, secondary).filter { it.isNotEmpty() }.joinToString(" · ")
    }

    /** TalkBack reads the state label, then the value, then the update time. */
    fun accessibility(snapshot: NativeWidgetSnapshot, title: String, now: Long = System.currentTimeMillis(), zone: TimeZone = TimeZone.getDefault()): String {
        val summary = snapshot.summary
        if (summary == null || snapshot.sharingState == SharingState.ENDED) return "$title. $SHARING_ENDED. $SHARING_ENDED_DETAIL"
        val state = when (summary.primaryValue.state) {
            MetricState.FAILED -> "Couldn’t refresh. "
            MetricState.AUTH_EXPIRED -> "Authentication expired. "
            else -> ""
        }
        val updated = (summary.dataUpdatedAt ?: snapshot.refreshedAt)?.let { ". ${updated(it, now, zone)}" } ?: ""
        return "${summary.title}. $state${value(summary.primaryValue, now, zone)} ${supporting(summary, now, zone)}$updated"
    }

    /** Failed and expired values show the stale value dimmed, as the in-app card does. */
    fun stale(metric: MetricValue) = metric.state == MetricState.FAILED || metric.state == MetricState.AUTH_EXPIRED
}
