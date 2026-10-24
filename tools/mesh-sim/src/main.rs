//! BLE Mesh 网络模拟器 —— 在电脑上验证「百人房间」算法，不需要真机。
//!
//! 用法:
//!   cargo run --release -- --nodes 100 --neighbors 8 --ttl 8 --rounds 20
//!   cargo run --release -- --nodes 100 --scenario werewolf --report report.json
//!
//! 它模拟:
//!   - 随机几何图（节点按距离连边，模拟蓝牙 ~10m 半径）
//!   - 泛洪转发 + TTL + SeqNo 去重
//!   - 链路丢包 / 延迟 / 抖动退避
//!   - 统计端到端 P50/P95 延迟、覆盖率、冗余流量

mod net;
mod proto;
mod sim;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "mesh-sim", about = "BLE Mesh 网络模拟器 (Bluetooth Arena 算法验证)")]
struct Args {
    /// 节点数量
    #[arg(long, default_value_t = 100)]
    nodes: usize,

    /// 每个节点期望邻居数
    #[arg(long, default_value_t = 8)]
    neighbors: usize,

    /// 部署区域边长（米），用于推导密度
    #[arg(long, default_value_t = 60.0)]
    area: f64,

    /// 蓝牙通信半径（米）
    #[arg(long, default_value_t = 12.0)]
    radius: f64,

    /// 消息 TTL
    #[arg(long, default_value_t = 8)]
    ttl: u8,

    /// 链路丢包率 0.0~1.0
    #[arg(long, default_value_t = 0.08)]
    loss: f64,

    /// 每条链路基础延迟（毫秒）
    #[arg(long, default_value_t = 25.0)]
    link_delay_ms: f64,

    /// 广播轮数
    #[arg(long, default_value_t = 30)]
    rounds: usize,

    /// 场景: flood(纯泛洪) | werewolf(狼人杀发言) | chess(走子广播)
    #[arg(long, default_value = "flood")]
    scenario: String,

    /// 是否启用「RSSI 加权抖动退避」优化
    #[arg(long, default_value_t = true)]
    jitter: bool,

    /// 输出 JSON 报告路径
    #[arg(long)]
    report: Option<PathBuf>,

    /// 随机种子（可复现）
    #[arg(long, default_value_t = 20251025)]
    seed: u64,
}

fn main() {
    let args = Args::parse();

    let cfg = sim::SimConfig {
        nodes: args.nodes,
        neighbors: args.neighbors,
        area: args.area,
        radius: args.radius,
        ttl: args.ttl,
        loss: args.loss,
        link_delay_ms: args.link_delay_ms,
        rounds: args.rounds,
        jitter: args.jitter,
        seed: args.seed,
    };

    println!("📶 BLE Mesh 模拟器 · Bluetooth Arena");
    println!(
        "   节点 {} | 期望邻居 {} | 半径 {:.1}m | TTL {} | 丢包 {:.0}%",
        cfg.nodes,
        cfg.neighbors,
        cfg.radius,
        cfg.ttl,
        cfg.loss * 100.0
    );

    let mut network = net::Network::build(&cfg);
    println!(
        "   拓扑: 平均邻居 {:.1} | 连通分量 {} | 最大分量占比 {:.0}%",
        network.avg_degree(),
        network.components(),
        network.largest_component_ratio() * 100.0
    );

    let result = match args.scenario.as_str() {
        "werewolf" => sim::run_werewolf(&mut network, &cfg),
        "chess" => sim::run_chess(&mut network, &cfg),
        _ => sim::run_flood(&mut network, &cfg),
    };

    result.print(&args.scenario);

    if let Some(path) = args.report {
        let json = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(&path, json).expect("写报告失败");
        println!("\n   报告已写入 {}", path.display());
    }
}
