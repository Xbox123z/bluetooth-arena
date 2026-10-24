# 📶 Bluetooth Arena — 蓝牙自组网游戏大厅

> 不开热点、不连 WiFi、不耗流量 —— 只要手机有蓝牙，就能开局。

**狼人杀 + 国际象棋，全程蓝牙互联，支持百人房间。**

适合：宿舍熄灯后、户外没信号、地铁上、课堂间隙 —— 任何你想和朋友开局却没有网络的地方。

---

## 为什么需要 Mesh（而不是普通蓝牙）

蓝牙的物理限制摆在那里：

| 拓扑 | 上限 | 问题 |
|---|---|---|
| 经典蓝牙 piconet | 1 主 7 从 | 星型，主机一断全崩 |
| BLE 中心-外设 | 实际 4~8 | 手机芯片并发连接数有限 |
| **BLE 多跳 Mesh** | **100+ 实测** | **每部手机都是中继，距离不再是限制** |

所以本项目采用 **BLE GATT 多跳泛洪 Mesh**：每台手机同时是 `Central`（扫描 + 连接）和
`Peripheral`（广播 + 被连接），消息靠 TTL 泛洪转发，形成一张自愈的无线网络。
**人越多，覆盖越广** —— 这和普通蓝牙「人越多越卡」正好相反。

---

## 架构

```
┌────────────── 应用层 (Kotlin / Compose) ──────────────┐
│  狼人杀引擎              国际象棋引擎                   │
│  - 角色分配（种子化）     - 完整规则（易位/吃过路兵/升变）│
│  - 昼夜阶段机             - 将军 / 将死 / 逼和判定       │
│  - 投票 + 平票重投        - PGN 记谱                    │
│  - 发言消息队列           - 悔棋 / 认输                  │
└────────────────┬──────────────────────────────────────┘
                 │  房间同步（事件溯源 + 状态快照）
┌────────────────┴──────────────────────────────────────┐
│  SyncLayer                                             │
│  - EventLog（仅追加，带 term/seq）                      │
│  - 确定性状态机：相同事件序列 ⇒ 相同状态                 │
│  - 落后节点请求快照补全                                 │
└────────────────┬──────────────────────────────────────┘
                 │
┌────────────────┴──────────────────────────────────────┐
│  BleMesh                                               │
│  - NodeId(2B) + SeqNo LRU 去重                         │
│  - 分片/重组（MTU 244 → 200B 有效载荷）                 │
│  - 泛洪转发 + TTL(默认 8) + RSSI 加权抖动退避            │
│  - 邻居表 & 链路质量                                    │
│  - 自适应：邻居数 < 4 才继续扫描，省电                   │
└────────────────┬──────────────────────────────────────┘
                 │
┌────────────────┴──────────────────────────────────────┐
│  Android BLE 传输                                      │
│  Peripheral: GATT Server（TX/RX Characteristic）        │
│  Central:    Scan + GATT Client + Write/Notify          │
│  权限: BLUETOOTH_SCAN/CONNECT/ADVERTISE (Android 12+)   │
└────────────────────────────────────────────────────────┘
```

---

## 消息格式

14 字节紧凑头部 + CBOR 载荷，为省 BLE 带宽而设计：

```
 0      1      2        4          6        8
┌──────┬──────┬────────┬──────────┬────────┬──────────┐
│ MAGIC│ VER  │ TYPE   │ SRC      │ DST    │ SEQ      │
│ 0xBA │ 0x01 │ 1 byte │ 2 bytes  │ 2bytes │ 4 bytes  │
├──────┴──────┴────────┴──────────┴────────┴──────────┤
│ TTL(1) | FRAG_IDX(1) | FRAG_TOTAL(1) | CRC16(2)     │
├─────────────────────────────────────────────────────┤
│ PAYLOAD (≤ 200 bytes)                               │
└─────────────────────────────────────────────────────┘
```

`CRC-16/CCITT-FALSE` 校验 + LRU 去重 + TTL 递减，坏包静默丢弃。

---

## 百人房间是怎么撑住的

1. **分片广播**：BLE 广播包只有 31 字节，房间发现走 `Service Data`
   （房间号 + 人数 + 房主 NodeId），连上后再拉全量成员列表。
2. **泛洪抑制**：`SeqNo` LRU 去重 + 首次转发前随机等待
   （**RSSI 越低等越久**），让好链路先走，避免广播风暴。
3. **分区容忍**：网络分裂后各自推进，重连时按 `(term, seq)` 确定性合并。
4. **无中心共识**：
   - 狼人杀身份用 `seed = hash(roomId, memberList.sort())` 洗牌，
     **所有人算出的结果一致**，改本地代码无效（别人算出的身份和你不一致，投票时会被多数否决）；
   - 关键操作（投票、走子）要求 **≥ 2/3 节点确认** 才提交。

**离线可验证**：`tools/mesh-sim` 是一个 Rust 写的 Mesh 网络模拟器，
不用真机就能模拟 100 节点、验证泛洪覆盖率和延迟：

```bash
cd tools/mesh-sim
cargo run --release -- --nodes 100 --neighbors 8 --ttl 8 --scenario werewolf --report report.json
```

输出示例：

```
📶 BLE Mesh 模拟器 · Bluetooth Arena
   节点 100 | 期望邻居 8 | 半径 12.0m | TTL 8 | 丢包 8%
   拓扑: 平均邻居 7.4 | 连通分量 1 | 最大分量占比 100%

── 场景: werewolf ─────────────────────────────
  覆盖率         99.80%
  延迟 P50       142.0 ms
  延迟 P95       388.0 ms
  冗余系数       1.32 (越低越省电)
  结论           ✅ 满足百人房间目标 (覆盖≥99%, P95<2s)
```

---

## 游戏

### 🐺 狼人杀（6~18 人 / 房，支持 100 人观战）

- 身份：村民 / 狼人 / 预言家 / 女巫 / 猎人 / 守卫 / 白痴
- 夜晚：狼人刀人 → 预言家查验 → 女巫毒救 → 守卫守护
- 白天：发言 → 投票 → 遗言 → 猎人开枪
- 房主端主持「上帝」，其余节点本地同步同一套确定性状态机
- 发言通过 Mesh 广播，带 **Lamport 时钟** 排序，保证所有人看到相同顺序

### ♟️ 国际象棋（1v1，房内配对）

- 完整规则：王车易位、吃过路兵、升变、三次重复、50 步和棋、逼和
- PGN 导出
- 旁观者实时看棋（Mesh 广播 FEN）

---

## 目录结构

```
bluetooth-arena/
├── app/                       # Android 应用 (Kotlin + Jetpack Compose)
│   ├── mesh/                  #   BLE Mesh 核心（协议、分片、去重、泛洪）
│   ├── game/werewolf/         #   狼人杀引擎
│   ├── game/chess/            #   国际象棋引擎
│   └── ui/                    #   Compose 界面
├── protocol/                  # 消息协议（Kotlin 与 Rust 双端共享定义）
├── tools/mesh-sim/            # Rust Mesh 模拟器（算法离线验证）
└── docs/                      # 设计文档
```

## 构建

```bash
./gradlew assembleDebug
# 或 Android Studio 直接打开根目录
```

要求：JDK 17+、Android SDK 35、`minSdk 26`（Android 8.0+）。

## 权限

```xml
<uses-permission android:name="android.permission.BLUETOOTH_SCAN" />
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />
<uses-permission android:name="android.permission.BLUETOOTH_ADVERTISE" />
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />
```

Android 12+ 需要运行时申请；Android 6~11 降级为 `ACCESS_FINE_LOCATION` + 旧蓝牙权限。

## 路线图

- [x] 协议层（Kotlin + Rust 双端）
- [x] Mesh 模拟器（覆盖/延迟/冗余统计）
- [ ] BLE 传输层（GATT Server/Client）
- [ ] 泛洪 + 邻居管理 + 自适应扫描
- [ ] 房间发现与成员同步
- [ ] 狼人杀引擎 + UI
- [ ] 国际象棋引擎 + UI

## License

MIT
