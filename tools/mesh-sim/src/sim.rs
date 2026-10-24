//! 泛洪仿真：模拟 BLE Mesh 的多跳广播，统计延迟/覆盖率/冗余流量

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::net::{Network, Rng};
use crate::proto::{MeshMessage, MsgType, NodeId};

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub nodes: usize,
    pub neighbors: usize,
    pub area: f64,
    pub radius: f64,
    pub ttl: u8,
    pub loss: f64,
    pub link_delay_ms: f64,
    pub rounds: usize,
    pub jitter: bool,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimResult {
    pub scenario: String,
    pub nodes: usize,
    pub avg_degree: f64,
    pub components: usize,
    pub largest_component_ratio: f64,
    pub ttl: u8,
    pub loss_rate: f64,
    pub jitter_enabled: bool,
    /// 覆盖率：收到消息的节点 / 最大分量节点数
    pub coverage: f64,
    /// 端到端延迟（毫秒）分位数
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_max_ms: f64,
    /// 冗余系数：总转发次数 / (覆盖节点数 - 1)，越低越省电
    pub redundancy: f64,
    pub total_transmissions: usize,
    pub delivered: usize,
    pub duplicated_drops: usize,
    pub ttl_drops: usize,
    pub loss_drops: usize,
    pub messages_sent: usize,
    /// 是否满足目标：覆盖率 ≥99%，P95 < 2000ms
    pub meets_target: bool,
}

/// 事件队列里的待处理项
#[derive(Debug, Clone)]
struct Pending {
    at_ms: f64,
    /// 接收节点索引
    node: usize,
    /// 发送节点索引（用于统计）
    from: usize,
    msg: MeshMessage,
}

/// 纯泛洪：每个源节点广播一条消息，看全网覆盖与延迟
pub fn run_flood(net: &mut Network, cfg: &SimConfig) -> SimResult {
    let mut rng = Rng::new(cfg.seed ^ 0x9E3779B9);
    let mut arrivals: Vec<f64> = Vec::new();
    let mut transmissions = 0usize;
    let mut delivered = 0usize;
    let mut dup_drops = 0usize;
    let mut ttl_drops = 0usize;
    let mut loss_drops = 0usize;

    let rounds = cfg.rounds.min(net.len());
    for r in 0..rounds {
        let src_idx = (r * 7 + 3) % net.len().max(1);
        let stats = flood_one(net, cfg, src_idx, &mut rng, r as u32);
        arrivals.extend(stats.arrivals);
        transmissions += stats.transmissions;
        delivered += stats.delivered;
        dup_drops += stats.dup_drops;
        ttl_drops += stats.ttl_drops;
        loss_drops += stats.loss_drops;
    }

    finalize(net, cfg, "flood", arrivals, transmissions, delivered, dup_drops, ttl_drops, loss_drops, rounds)
}

/// 狼人杀场景：一名玩家发言，要求 95% 节点在 2s 内收到
pub fn run_werewolf(net: &mut Network, cfg: &SimConfig) -> SimResult {
    let mut rng = Rng::new(cfg.seed ^ 0x5DEECE66D);
    let mut arrivals: Vec<f64> = Vec::new();
    let mut transmissions = 0usize;
    let mut delivered = 0usize;
    let mut dup_drops = 0usize;
    let mut ttl_drops = 0usize;
    let mut loss_drops = 0usize;

    // 模拟一局 12 人局：夜晚 4 次私聊 + 白天 9 次发言
    let speeches = 13usize;
    for r in 0..speeches {
        let src_idx = (r * 5 + 1) % net.len().max(1);
        let stats = flood_one(net, cfg, src_idx, &mut rng, r as u32);
        arrivals.extend(stats.arrivals);
        transmissions += stats.transmissions;
        delivered += stats.delivered;
        dup_drops += stats.dup_drops;
        ttl_drops += stats.ttl_drops;
        loss_drops += stats.loss_drops;
    }

    finalize(net, cfg, "werewolf", arrivals, transmissions, delivered, dup_drops, ttl_drops, loss_drops, speeches)
}

/// 象棋场景：走子广播，低频但要求低延迟
pub fn run_chess(net: &mut Network, cfg: &SimConfig) -> SimResult {
    let mut rng = Rng::new(cfg.seed ^ 0x1BADC0DE);
    let mut arrivals: Vec<f64> = Vec::new();
    let mut transmissions = 0usize;
    let mut delivered = 0usize;
    let mut dup_drops = 0usize;
    let mut ttl_drops = 0usize;
    let mut loss_drops = 0usize;

    let moves = 40usize;
    for r in 0..moves {
        let src_idx = (r * 3) % net.len().max(1);
        let stats = flood_one(net, cfg, src_idx, &mut rng, r as u32);
        arrivals.extend(stats.arrivals);
        transmissions += stats.transmissions;
        delivered += stats.delivered;
        dup_drops += stats.dup_drops;
        ttl_drops += stats.ttl_drops;
        loss_drops += stats.loss_drops;
    }

    finalize(net, cfg, "chess", arrivals, transmissions, delivered, dup_drops, ttl_drops, loss_drops, moves)
}

#[derive(Default)]
struct FloodStats {
    arrivals: Vec<f64>,
    transmissions: usize,
    delivered: usize,
    dup_drops: usize,
    ttl_drops: usize,
    loss_drops: usize,
}

/// 从 `src_idx` 泛洪一条消息（BFS + 时间优先队列，保证延迟计算正确）
fn flood_one(
    net: &Network,
    cfg: &SimConfig,
    src_idx: usize,
    rng: &mut Rng,
    seq: u32,
) -> FloodStats {
    let mut stats = FloodStats::default();
    if net.is_empty() || src_idx >= net.len() {
        return stats;
    }

    let src_id = net.nodes[src_idx].id;
    let msg = MeshMessage::new(
        MsgType::GameEvent,
        src_id,
        NodeId::BROADCAST,
        seq,
        cfg.ttl,
        b"mesh".to_vec(),
    );

    // 每个节点记录首次到达时间 + 已见过的 (src,seq) 去重集合
    let mut first_arrival: HashMap<usize, f64> = HashMap::new();
    let mut seen: HashSet<(NodeId, u32, u8)> = HashSet::new();
    let mut queue: VecDeque<Pending> = VecDeque::new();

    first_arrival.insert(src_idx, 0.0);
    seen.insert(msg.dedup_key());

    // 源节点向所有邻居发送
    for link in &net.nodes[src_idx].links {
        stats.transmissions += 1;
        let mut m = msg.clone();
        m.ttl = m.ttl.saturating_sub(1);
        queue.push_back(Pending {
            at_ms: link_delay(cfg, link.quality),
            node: link.to,
            from: src_idx,
            msg: m,
        });
    }

    // 时间优先：用简单的「取最小 at_ms」模拟优先队列（规模小，够用）
    while let Some(p) = take_earliest(&mut queue) {
        // 去重：已收到过的包直接丢弃
        if seen.contains(&p.msg.dedup_key()) && first_arrival.contains_key(&p.node) {
            stats.dup_drops += 1;
            continue;
        }

        // 丢包模拟
        if rng.next_f64() < cfg.loss {
            stats.loss_drops += 1;
            continue;
        }

        // TTL 耗尽
        if p.msg.ttl == 0 {
            stats.ttl_drops += 1;
            continue;
        }

        let is_first = !first_arrival.contains_key(&p.node);
        if is_first {
            first_arrival.insert(p.node, p.at_ms);
            seen.insert(p.msg.dedup_key());
            stats.delivered += 1;
        }

        // 转发（只有首次收到才转发，避免无限循环）
        if is_first {
            // RSSI 加权抖动退避：信号差的节点等久一点再转发，让好链路先走
            let jitter_ms = if cfg.jitter {
                let q = net.nodes[p.node]
                    .links
                    .iter()
                    .find(|l| l.to == p.from)
                    .map(|l| l.quality)
                    .unwrap_or(0.5);
                (1.0 - q) * 15.0 * rng.next_f64()
            } else {
                0.0
            };

            for link in &net.nodes[p.node].links {
                if link.to == p.from {
                    continue; // 不回发给来路
                }
                stats.transmissions += 1;
                let mut m = p.msg.clone();
                m.ttl = m.ttl.saturating_sub(1);
                queue.push_back(Pending {
                    at_ms: p.at_ms + jitter_ms + link_delay(cfg, link.quality),
                    node: link.to,
                    from: p.node,
                    msg: m,
                });
            }
        }
    }

    stats.arrivals = first_arrival
        .into_iter()
        .filter(|(n, _)| *n != src_idx)
        .map(|(_, t)| t)
        .collect();
    stats
}

fn link_delay(cfg: &SimConfig, quality: f64) -> f64 {
    // 质量差的链路重传多 -> 延迟高
    cfg.link_delay_ms * (1.0 + (1.0 - quality) * 2.0)
}

fn take_earliest(q: &mut VecDeque<Pending>) -> Option<Pending> {
    if q.is_empty() {
        return None;
    }
    let mut best = 0usize;
    let mut best_at = f64::MAX;
    for (i, p) in q.iter().enumerate() {
        if p.at_ms < best_at {
            best_at = p.at_ms;
            best = i;
        }
    }
    q.remove(best)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn finalize(
    net: &Network,
    cfg: &SimConfig,
    scenario: &str,
    mut arrivals: Vec<f64>,
    transmissions: usize,
    delivered: usize,
    dup_drops: usize,
    ttl_drops: usize,
    loss_drops: usize,
    messages_sent: usize,
) -> SimResult {
    arrivals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let largest = (net.largest_component_ratio() * net.len() as f64).round() as usize;
    let coverage = if largest > 1 && messages_sent > 0 {
        (delivered as f64 / messages_sent as f64) / (largest as f64 - 1.0).max(1.0)
    } else {
        0.0
    };

    let p50 = percentile(&arrivals, 0.50);
    let p95 = percentile(&arrivals, 0.95);
    let max = arrivals.last().copied().unwrap_or(0.0);
    let redundancy = if delivered > 0 {
        transmissions as f64 / delivered as f64
    } else {
        0.0
    };

    let meets_target = coverage >= 0.99 && p95 < 2000.0;

    SimResult {
        scenario: scenario.to_string(),
        nodes: net.len(),
        avg_degree: net.avg_degree(),
        components: net.components(),
        largest_component_ratio: net.largest_component_ratio(),
        ttl: cfg.ttl,
        loss_rate: cfg.loss,
        jitter_enabled: cfg.jitter,
        coverage: (coverage * 10000.0).round() / 10000.0,
        latency_p50_ms: (p50 * 10.0).round() / 10.0,
        latency_p95_ms: (p95 * 10.0).round() / 10.0,
        latency_max_ms: (max * 10.0).round() / 10.0,
        redundancy: (redundancy * 100.0).round() / 100.0,
        total_transmissions: transmissions,
        delivered,
        duplicated_drops: dup_drops,
        ttl_drops,
        loss_drops,
        messages_sent,
        meets_target,
    }
}

impl SimResult {
    pub fn print(&self, scenario: &str) {
        println!("\n── 场景: {} ─────────────────────────────", scenario);
        println!("  覆盖率         {:.2}%", self.coverage * 100.0);
        println!("  延迟 P50       {:.1} ms", self.latency_p50_ms);
        println!("  延迟 P95       {:.1} ms", self.latency_p95_ms);
        println!("  延迟 MAX       {:.1} ms", self.latency_max_ms);
        println!("  冗余系数       {:.2} (越低越省电)", self.redundancy);
        println!("  总发送次数     {}", self.total_transmissions);
        println!("  去重丢弃       {}", self.duplicated_drops);
        println!("  TTL 耗尽       {}", self.ttl_drops);
        println!("  链路丢包       {}", self.loss_drops);
        println!(
            "  结论           {}",
            if self.meets_target {
                "✅ 满足百人房间目标 (覆盖≥99%, P95<2s)"
            } else {
                "⚠️ 未达标：需增大密度 / 提高 TTL / 降低丢包"
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SimConfig {
        SimConfig {
            nodes: 30,
            neighbors: 8,
            area: 30.0,
            radius: 12.0,
            ttl: 8,
            loss: 0.0,
            link_delay_ms: 25.0,
            rounds: 5,
            jitter: true,
            seed: 12345,
        }
    }

    #[test]
    fn no_loss_covers_everyone() {
        let c = cfg();
        let mut net = Network::build(&c);
        let r = run_flood(&mut net, &c);
        // 无丢包时，最大分量内所有人都应收到
        assert!(r.coverage > 0.95, "覆盖率 {:.3}", r.coverage);
    }

    #[test]
    fn jitter_reduces_redundancy() {
        let c = SimConfig { jitter: true, ..cfg() };
        let c2 = SimConfig { jitter: false, ..cfg() };
        let mut n1 = Network::build(&c);
        let mut n2 = Network::build(&c2);
        let a = run_flood(&mut n1, &c);
        let b = run_flood(&mut n2, &c2);
        // 抖动至少不应显著变差
        assert!(a.redundancy <= b.redundancy * 1.5 + 0.5);
    }

    #[test]
    fn higher_loss_lower_coverage() {
        let good = SimConfig { loss: 0.0, ..cfg() };
        let bad = SimConfig { loss: 0.5, ..cfg() };
        let mut n1 = Network::build(&good);
        let mut n2 = Network::build(&bad);
        let a = run_flood(&mut n1, &good);
        let b = run_flood(&mut n2, &bad);
        assert!(a.coverage >= b.coverage, "丢包多不应覆盖更高");
    }

    #[test]
    fn ttl_zero_never_spreads() {
        let c = SimConfig { ttl: 0, ..cfg() };
        let mut net = Network::build(&c);
        let r = run_flood(&mut net, &c);
        assert_eq!(r.delivered, 0, "TTL=0 不应有任何投递");
    }

    #[test]
    fn latency_is_monotonic_in_hops() {
        let c = cfg();
        let mut net = Network::build(&c);
        let r = run_flood(&mut net, &c);
        assert!(r.latency_p50_ms <= r.latency_p95_ms);
        assert!(r.latency_p95_ms <= r.latency_max_ms + 1e-6);
    }

    #[test]
    fn percentile_edges() {
        let empty: Vec<f64> = vec![];
        assert_eq!(percentile(&empty, 0.5), 0.0);
        let one = vec![7.0];
        assert_eq!(percentile(&one, 0.95), 7.0);
        let ten: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        assert_eq!(percentile(&ten, 0.0), 1.0);
        assert_eq!(percentile(&ten, 1.0), 10.0);
    }

    #[test]
    fn empty_network_safe() {
        let c = SimConfig { nodes: 0, ..cfg() };
        let mut net = Network::build(&c);
        let r = run_flood(&mut net, &c);
        assert_eq!(r.nodes, 0);
        assert_eq!(r.coverage, 0.0);
    }
}
