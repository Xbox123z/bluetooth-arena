//! 网络拓扑：随机几何图（模拟蓝牙 ~10m 半径的真实分布）

use crate::proto::NodeId;
use crate::sim::SimConfig;

/// 确定性 LCG，保证同种子结果可复现
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn dist(&self, o: &Point) -> f64 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        (dx * dx + dy * dy).sqrt()
    }
}

#[derive(Debug, Clone)]
pub struct Link {
    pub to: usize,
    /// 链路质量 0.0~1.0，由距离推导（近 = 好）
    pub quality: f64,
    pub rssi_dbm: i8,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub pos: Point,
    pub links: Vec<Link>,
}

#[derive(Debug)]
pub struct Network {
    pub nodes: Vec<Node>,
    pub radius: f64,
}

impl Network {
    /// 在 `area × area` 的场地里撒点，距离 ≤ radius 的连边
    pub fn build(cfg: &SimConfig) -> Self {
        let mut rng = Rng::new(cfg.seed);
        let mut nodes = Vec::with_capacity(cfg.nodes);

        for i in 0..cfg.nodes {
            let pos = Point {
                x: rng.range(0.0, cfg.area),
                y: rng.range(0.0, cfg.area),
            };
            // NodeId 从 1 开始，0 保留
            nodes.push(Node {
                id: NodeId((i as u16).wrapping_add(1)),
                pos,
                links: Vec::new(),
            });
        }

        // 连边：O(n^2) 对 100 节点完全够用
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                let d = nodes[i].pos.dist(&nodes[j].pos);
                if d <= cfg.radius {
                    let quality = link_quality(d, cfg.radius);
                    let rssi = rssi_from_dist(d);
                    nodes[i].links.push(Link { to: j, quality, rssi_dbm: rssi });
                    nodes[j].links.push(Link { to: i, quality, rssi_dbm: rssi });
                }
            }
        }

        // 度数上限：真实手机同时维持的 BLE 连接有限
        let cap = cfg.neighbors.max(2);
        for n in nodes.iter_mut() {
            if n.links.len() > cap {
                // 保留质量最好的 cap 条（对称处理会略有偏差，模拟器可接受）
                n.links.sort_by(|a, b| b.quality.partial_cmp(&a.quality).unwrap());
                n.links.truncate(cap);
            }
        }

        Self { nodes, radius: cfg.radius }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn avg_degree(&self) -> f64 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        let total: usize = self.nodes.iter().map(|n| n.links.len()).sum();
        total as f64 / self.nodes.len() as f64
    }

    /// 连通分量个数（并查集）
    pub fn components(&self) -> usize {
        let n = self.nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();

        fn find(parent: &mut [usize], x: usize) -> usize {
            let mut r = x;
            while parent[r] != r {
                r = parent[r];
            }
            let mut cur = x;
            while parent[cur] != r {
                let next = parent[cur];
                parent[cur] = r;
                cur = next;
            }
            r
        }

        for i in 0..n {
            for l in &self.nodes[i].links {
                let a = find(&mut parent, i);
                let b = find(&mut parent, l.to);
                if a != b {
                    parent[a] = b;
                }
            }
        }
        let mut roots: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len()
    }

    /// 最大连通分量占比 —— 决定「百人房间」是否真的能互通
    pub fn largest_component_ratio(&self) -> f64 {
        let n = self.nodes.len();
        if n == 0 {
            return 0.0;
        }
        let mut visited = vec![false; n];
        let mut best = 0usize;

        for start in 0..n {
            if visited[start] {
                continue;
            }
            let mut stack = vec![start];
            let mut count = 0usize;
            visited[start] = true;
            while let Some(v) = stack.pop() {
                count += 1;
                for l in &self.nodes[v].links {
                    if !visited[l.to] {
                        visited[l.to] = true;
                        stack.push(l.to);
                    }
                }
            }
            best = best.max(count);
        }
        best as f64 / n as f64
    }

    /// 直径估计：从节点 0 做 BFS 取最大跳数作为下界
    pub fn hop_diameter_estimate(&self) -> usize {
        let n = self.nodes.len();
        if n == 0 {
            return 0;
        }
        let mut dist = vec![usize::MAX; n];
        let mut q = std::collections::VecDeque::new();
        dist[0] = 0;
        q.push_back(0);
        while let Some(v) = q.pop_front() {
            for l in &self.nodes[v].links {
                if dist[l.to] == usize::MAX {
                    dist[l.to] = dist[v] + 1;
                    q.push_back(l.to);
                }
            }
        }
        dist.iter().filter(|d| **d != usize::MAX).copied().max().unwrap_or(0)
    }
}

/// 距离 -> 链路质量（1.0 贴脸，0 边缘）
pub fn link_quality(d: f64, radius: f64) -> f64 {
    if d >= radius {
        return 0.0;
    }
    let t = d / radius;
    // 近处衰减慢，边缘快速变差
    (1.0 - t * t).max(0.05)
}

/// 距离 -> RSSI（对数距离路径损耗模型的简化）
pub fn rssi_from_dist(d: f64) -> i8 {
    // RSSI ≈ -40 - 25*log10(d)，d<1m 视为 1m
    let dd = d.max(1.0);
    let r = -40.0 - 25.0 * dd.log10();
    r.clamp(-100.0, -30.0) as i8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(nodes: usize, seed: u64) -> SimConfig {
        SimConfig {
            nodes,
            neighbors: 8,
            area: 60.0,
            radius: 12.0,
            ttl: 8,
            loss: 0.08,
            link_delay_ms: 25.0,
            rounds: 10,
            jitter: true,
            seed,
        }
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn rng_in_unit_range() {
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn build_respects_node_count() {
        let net = Network::build(&cfg(50, 1));
        assert_eq!(net.len(), 50);
    }

    #[test]
    fn degree_capped() {
        let net = Network::build(&cfg(80, 3));
        for n in &net.nodes {
            assert!(n.links.len() <= 8, "度数应被 cap 限制");
        }
    }

    #[test]
    fn dense_network_is_connected() {
        // 60 个节点挤在 25x25 里，半径 12 -> 应该基本全连通
        let c = SimConfig { nodes: 60, area: 25.0, ..cfg(60, 5) };
        let net = Network::build(&c);
        assert!(
            net.largest_component_ratio() > 0.9,
            "密集网络应几乎全连通, 实际 {:.2}",
            net.largest_component_ratio()
        );
    }

    #[test]
    fn sparse_network_splits() {
        let c = SimConfig { nodes: 40, area: 400.0, radius: 12.0, ..cfg(40, 9) };
        let net = Network::build(&c);
        assert!(net.components() > 1, "稀疏网络应分裂成多个分量");
    }

    #[test]
    fn link_quality_monotonic() {
        assert!(link_quality(1.0, 12.0) > link_quality(6.0, 12.0));
        assert!(link_quality(6.0, 12.0) > link_quality(11.0, 12.0));
        assert_eq!(link_quality(12.0, 12.0), 0.0);
        assert_eq!(link_quality(20.0, 12.0), 0.0);
    }

    #[test]
    fn rssi_bounded() {
        for d in [0.1, 1.0, 5.0, 20.0, 1000.0] {
            let r = rssi_from_dist(d);
            assert!((-100..=-30).contains(&r));
        }
    }

    #[test]
    fn distance_symmetric() {
        let a = Point { x: 0.0, y: 0.0 };
        let b = Point { x: 3.0, y: 4.0 };
        assert!((a.dist(&b) - 5.0).abs() < 1e-9);
        assert!((a.dist(&b) - b.dist(&a)).abs() < 1e-9);
    }
}
