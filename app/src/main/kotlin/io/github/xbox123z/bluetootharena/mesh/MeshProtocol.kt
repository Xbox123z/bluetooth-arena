package io.github.xbox123z.bluetootharena.mesh

/**
 * Mesh 消息协议 —— 与 tools/mesh-sim/src/proto.rs 一一对应。
 *
 * 线格式（14 字节头 + payload）：
 * ```
 * 0      1      2        4          6        8
 * ┌──────┬──────┬────────┬──────────┬────────┬──────────┐
 * │ MAGIC│ VER  │ TYPE   │ SRC      │ DST    │ SEQ      │
 * │ 0xBA │ 0x01 │ 1 byte │ 2 bytes  │ 2bytes │ 4 bytes  │
 * ├──────┴──────┴────────┴──────────┴────────┴──────────┤
 * │ TTL(1) | FRAG_IDX(1) | FRAG_TOTAL(1) | CRC16(2)     │
 * └─────────────────────────────────────────────────────┘
 * ```
 */
object MeshProtocol {
    const val MAGIC: Byte = 0xBA.toByte()
    const val VERSION: Byte = 0x01
    const val HEADER_LEN = 14
    /** BLE 单包有效载荷上限（MTU 244 - ATT 开销） */
    const val MAX_PAYLOAD = 200

    const val SERVICE_UUID = "0000BEA1-0000-1000-8000-00805F9B34FB"
    const val TX_CHAR_UUID = "0000BEA2-0000-1000-8000-00805F9B34FB"
    const val RX_CHAR_UUID = "0000BEA3-0000-1000-8000-00805F9B34FB"
    const val CCCD_UUID = "00002902-0000-1000-8000-00805F9B34FB"

    /** 默认 TTL：100 节点、平均 8 邻居时足够覆盖全网 */
    const val DEFAULT_TTL: Int = 8
    /** 去重缓存容量（LRU） */
    const val DEDUP_CACHE_SIZE = 2048
}

@JvmInline
value class NodeId(val value: UShort) {
    companion object {
        val BROADCAST = NodeId(0xFFFFu)
        val NONE = NodeId(0x0000u)
    }

    fun isBroadcast() = value == BROADCAST.value
    override fun toString() = value.toString(16).padStart(4, '0').uppercase()
}

enum class MsgType(val id: Int) {
    Hello(0x01),
    NeighborSync(0x02),
    RoomAnnounce(0x03),
    RoomJoin(0x04),
    RoomMembers(0x05),
    GameEvent(0x10),
    SnapshotReq(0x20),
    SnapshotResp(0x21),
    Ping(0x30),
    Ack(0x31);

    companion object {
        fun fromId(id: Int): MsgType? = entries.firstOrNull { it.id == id }
    }
}

data class MeshMessage(
    val type: MsgType,
    val src: NodeId,
    val dst: NodeId,
    val seq: UInt,
    val ttl: UInt,
    val fragIdx: UInt = 0u,
    val fragTotal: UInt = 1u,
    val payload: ByteArray = ByteArray(0),
) {
    /** 序列化为线格式 */
    fun encode(): ByteArray {
        val out = ByteArray(MeshProtocol.HEADER_LEN + payload.size)
        var p = 0
        out[p++] = MeshProtocol.MAGIC
        out[p++] = MeshProtocol.VERSION
        out[p++] = type.id.toByte()
        writeU16BE(out, p, src.value); p += 2
        writeU16BE(out, p, dst.value); p += 2
        writeU32BE(out, p, seq); p += 4
        out[p++] = ttl.toByte()
        out[p++] = fragIdx.toByte()
        out[p++] = fragTotal.toByte()
        val crc = crc16(payload)
        writeU16BE(out, p, crc); p += 2
        payload.copyInto(out, p)
        return out
    }

    /** 分片：长 payload 拆成 ≤ MAX_PAYLOAD 的包 */
    fun fragment(): List<MeshMessage> {
        if (payload.size <= MeshProtocol.MAX_PAYLOAD) return listOf(this)
        val total = (payload.size + MeshProtocol.MAX_PAYLOAD - 1) / MeshProtocol.MAX_PAYLOAD
        return payload.toList().chunked(MeshProtocol.MAX_PAYLOAD).mapIndexed { i, chunk ->
            copy(fragIdx = i.toUInt(), fragTotal = total.toUInt(), payload = chunk.toByteArray())
        }
    }

    /** 去重键：同一 (src, seq, 分片号) 只处理一次 */
    fun dedupKey(): Long =
        (src.value.toLong() shl 48) or (seq.toLong() shl 16) or fragIdx.toLong()

    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (other !is MeshMessage) return false
        return type == other.type && src == other.src && dst == other.dst &&
            seq == other.seq && ttl == other.ttl && fragIdx == other.fragIdx &&
            fragTotal == other.fragTotal && payload.contentEquals(other.payload)
    }

    override fun hashCode(): Int {
        var r = type.hashCode()
        r = 31 * r + src.hashCode()
        r = 31 * r + dst.hashCode()
        r = 31 * r + seq.hashCode()
        r = 31 * r + ttl.hashCode()
        r = 31 * r + fragIdx.hashCode()
        r = 31 * r + payload.contentHashCode()
        return r
    }

    companion object {
        /** 解析；任何不合法返回 null（Mesh 里静默丢弃坏包） */
        fun decode(bytes: ByteArray): MeshMessage? {
            if (bytes.size < MeshProtocol.HEADER_LEN) return null
            if (bytes[0] != MeshProtocol.MAGIC || bytes[1] != MeshProtocol.VERSION) return null
            val type = MsgType.fromId(bytes[2].toInt() and 0xFF) ?: return null
            val src = NodeId(readU16BE(bytes, 3))
            val dst = NodeId(readU16BE(bytes, 5))
            val seq = readU32BE(bytes, 7)
            val ttl = bytes[11].toUInt()
            val fragIdx = bytes[12].toUInt()
            val fragTotal = bytes[13].toUInt()
            val crc = readU16BE(bytes, 14 - 2 + 2 - 2)
            val payload = bytes.copyOfRange(MeshProtocol.HEADER_LEN, bytes.size)
            if (crc16(payload) != crc) return null
            return MeshMessage(type, src, dst, seq, ttl, fragIdx, fragTotal, payload)
        }

        /** CRC-16/CCITT-FALSE */
        fun crc16(data: ByteArray): UShort {
            var crc: Int = 0xFFFF
            for (b in data) {
                crc = crc xor ((b.toInt() and 0xFF) shl 8)
                repeat(8) {
                    crc = if (crc and 0x8000 != 0) ((crc shl 1) xor 0x1021) and 0xFFFF else (crc shl 1) and 0xFFFF
                }
            }
            return crc.toUShort()
        }

        private fun writeU16BE(buf: ByteArray, off: Int, v: UShort) {
            buf[off] = (v.toInt() shr 8).toByte()
            buf[off + 1] = (v.toInt() and 0xFF).toByte()
        }

        private fun writeU32BE(buf: ByteArray, off: Int, v: UInt) {
            buf[off] = (v.toInt() shr 24).toByte()
            buf[off + 1] = (v.toInt() shr 16).toByte()
            buf[off + 2] = (v.toInt() shr 8).toByte()
            buf[off + 3] = (v.toInt() and 0xFF).toByte()
        }

        private fun readU16BE(buf: ByteArray, off: Int): UShort =
            (((buf[off].toInt() and 0xFF) shl 8) or (buf[off + 1].toInt() and 0xFF)).toUShort()

        private fun readU32BE(buf: ByteArray, off: Int): UInt =
            (((buf[off].toInt() and 0xFF) shl 24) or
                ((buf[off + 1].toInt() and 0xFF) shl 16) or
                ((buf[off + 2].toInt() and 0xFF) shl 8) or
                (buf[off + 3].toInt() and 0xFF)).toUInt()
    }
}

/** 分片重组器：收集同一 (src, seq) 的全部分片 */
class FragmentReassembler {
    private data class Key(val src: UShort, val seq: UInt)

    private val pending = LinkedHashMap<Key, Array<ByteArray?>>()
    private val received = LinkedHashMap<Key, Int>()

    /** 返回完整 payload，或 null 表示还没集齐 */
    fun offer(msg: MeshMessage): ByteArray? {
        if (msg.fragTotal <= 1u) return msg.payload
        val key = Key(msg.src.value, msg.seq)
        val total = msg.fragTotal.toInt()
        val parts = pending.getOrPut(key) { arrayOfNulls(total) }
        if (msg.fragIdx.toInt() < total) {
            if (parts[msg.fragIdx.toInt()] == null) {
                parts[msg.fragIdx.toInt()] = msg.payload
                received[key] = (received[key] ?: 0) + 1
            }
        }
        if (received[key] == total) {
            val joined = parts.filterNotNull().fold(ByteArray(0)) { acc, b -> acc + b }
            pending.remove(key)
            received.remove(key)
            return joined
        }
        return null
    }

    /** 清理超时未集齐的分片 */
    fun prune(maxEntries: Int = 64) {
        while (pending.size > maxEntries) {
            val first = pending.keys.firstOrNull() ?: break
            pending.remove(first)
            received.remove(first)
        }
    }
}
