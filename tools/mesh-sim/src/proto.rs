//! Mesh 消息协议定义 —— 与 Android 端 `protocol/` 目录一一对应。

pub const MAGIC: u8 = 0xBA;
pub const VERSION: u8 = 0x01;

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    /// 节点上线广播
    Hello = 0x01,
    /// 邻居表同步
    NeighborSync = 0x02,
    /// 房间发现
    RoomAnnounce = 0x03,
    /// 加入房间请求
    RoomJoin = 0x04,
    /// 房间成员列表
    RoomMembers = 0x05,
    /// 游戏事件（狼人杀发言 / 象棋走子）
    GameEvent = 0x10,
    /// 状态快照请求
    SnapshotReq = 0x20,
    /// 状态快照响应
    SnapshotResp = 0x21,
    /// 心跳
    Ping = 0x30,
    /// 确认
    Ack = 0x31,
}

/// 线格式头部（14 字节）
///
/// ```text
/// 0      1      2        4          6        8
/// ┌──────┬──────┬────────┬──────────┬────────┬──────────┐
/// │ MAGIC│ VER  │ TYPE   │ SRC      │ DST    │ SEQ      │
/// │ 0xBA │ 0x01 │ 1 byte │ 2 bytes  │ 2bytes │ 4 bytes  │
/// ├──────┴──────┴────────┴──────────┴────────┴──────────┤
/// │ TTL(1) | FRAG_IDX(1) | FRAG_TOTAL(1) | CRC16(2)     │
/// └─────────────────────────────────────────────────────┘
/// ```
pub const HEADER_LEN: usize = 14;
/// BLE 单包有效载荷上限（MTU 244 - ATT 开销）
pub const MAX_PAYLOAD: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u16);

impl NodeId {
    pub const BROADCAST: NodeId = NodeId(0xFFFF);
}

#[derive(Debug, Clone)]
pub struct MeshMessage {
    pub msg_type: MsgType,
    pub src: NodeId,
    pub dst: NodeId,
    pub seq: u32,
    pub ttl: u8,
    pub frag_idx: u8,
    pub frag_total: u8,
    pub payload: Vec<u8>,
}

impl MeshMessage {
    pub fn new(msg_type: MsgType, src: NodeId, dst: NodeId, seq: u32, ttl: u8, payload: Vec<u8>) -> Self {
        Self {
            msg_type,
            src,
            dst,
            seq,
            ttl,
            frag_idx: 0,
            frag_total: 1,
            payload,
        }
    }

    /// 序列化为线格式
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(MAGIC);
        out.push(VERSION);
        out.push(self.msg_type as u8);
        out.extend_from_slice(&self.src.0.to_be_bytes());
        out.extend_from_slice(&self.dst.0.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.push(self.ttl);
        out.push(self.frag_idx);
        out.push(self.frag_total);
        let crc = crc16(&self.payload);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// 从线格式解析；任何不合法都返回 None（Mesh 里静默丢弃坏包）
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN || bytes[0] != MAGIC || bytes[1] != VERSION {
            return None;
        }
        let msg_type = match bytes[2] {
            x if x == MsgType::Hello as u8 => MsgType::Hello,
            x if x == MsgType::NeighborSync as u8 => MsgType::NeighborSync,
            x if x == MsgType::RoomAnnounce as u8 => MsgType::RoomAnnounce,
            x if x == MsgType::RoomJoin as u8 => MsgType::RoomJoin,
            x if x == MsgType::RoomMembers as u8 => MsgType::RoomMembers,
            x if x == MsgType::GameEvent as u8 => MsgType::GameEvent,
            x if x == MsgType::SnapshotReq as u8 => MsgType::SnapshotReq,
            x if x == MsgType::SnapshotResp as u8 => MsgType::SnapshotResp,
            x if x == MsgType::Ping as u8 => MsgType::Ping,
            x if x == MsgType::Ack as u8 => MsgType::Ack,
            _ => return None,
        };
        let src = NodeId(u16::from_be_bytes([bytes[3], bytes[4]]));
        let dst = NodeId(u16::from_be_bytes([bytes[5], bytes[6]]));
        let seq = u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]);
        let ttl = bytes[11];
        let frag_idx = bytes[12];
        let frag_total = bytes[13];
        let crc = u16::from_be_bytes([bytes[14], bytes[15]]);
        let payload = bytes[HEADER_LEN..].to_vec();
        if crc16(&payload) != crc {
            return None;
        }
        Some(Self {
            msg_type,
            src,
            dst,
            seq,
            ttl,
            frag_idx,
            frag_total,
            payload,
        })
    }

    /// 分片：把长 payload 拆成多个 ≤ MAX_PAYLOAD 的包
    pub fn fragment(self) -> Vec<Self> {
        if self.payload.len() <= MAX_PAYLOAD {
            return vec![self];
        }
        let chunks: Vec<Vec<u8>> = self
            .payload
            .chunks(MAX_PAYLOAD)
            .map(|c| c.to_vec())
            .collect();
        let total = chunks.len() as u8;
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, p)| Self {
                frag_idx: i as u8,
                frag_total: total,
                payload: p,
                ..self.clone()
            })
            .collect()
    }

    /// 唯一标识（用于去重缓存）
    pub fn dedup_key(&self) -> (NodeId, u32, u8) {
        (self.src, self.seq, self.frag_idx)
    }
}

/// CRC-16/CCITT-FALSE
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for b in data {
        crc ^= (*b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let m = MeshMessage::new(
            MsgType::GameEvent,
            NodeId(0x1234),
            NodeId::BROADCAST,
            42,
            8,
            b"hello mesh".to_vec(),
        );
        let bytes = m.encode();
        let back = MeshMessage::decode(&bytes).unwrap();
        assert_eq!(back.src, NodeId(0x1234));
        assert_eq!(back.seq, 42);
        assert_eq!(back.ttl, 8);
        assert_eq!(back.payload, b"hello mesh".to_vec());
        assert_eq!(back.msg_type, MsgType::GameEvent);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut m = MeshMessage::new(MsgType::Ping, NodeId(1), NodeId::BROADCAST, 1, 1, vec![1]);
        let mut bytes = m.encode();
        bytes[0] = 0x00;
        assert!(MeshMessage::decode(&bytes).is_none());
        let _ = &mut m;
    }

    #[test]
    fn crc_detects_corruption() {
        let m = MeshMessage::new(MsgType::Ack, NodeId(1), NodeId(2), 1, 1, vec![1, 2, 3]);
        let mut bytes = m.encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(MeshMessage::decode(&bytes).is_none());
    }

    #[test]
    fn fragmentation() {
        let big = vec![7u8; MAX_PAYLOAD * 3 + 10];
        let m = MeshMessage::new(MsgType::SnapshotResp, NodeId(1), NodeId(2), 9, 8, big.clone());
        let frags = m.fragment();
        assert_eq!(frags.len(), 4);
        let rejoined: Vec<u8> = frags.iter().flat_map(|f| f.payload.clone()).collect();
        assert_eq!(rejoined, big);
        for (i, f) in frags.iter().enumerate() {
            assert_eq!(f.frag_idx, i as u8);
            assert_eq!(f.frag_total, 4);
        }
    }

    #[test]
    fn header_len_is_14() {
        let m = MeshMessage::new(MsgType::Hello, NodeId(1), NodeId::BROADCAST, 0, 1, vec![]);
        assert_eq!(m.encode().len(), HEADER_LEN);
    }
}
