//! Measures pushing a 40-commit, ~300 KiB-of-history repository to an empty
//! remote across networks of increasing hostility (virtual ticks, one seed
//! per row so it is reproducible):
//!   cargo run --release -p sync --example measure

use object::ObjectId;
use repo::{Files, Repo};
use sync::{push, DistransRemote, NetConfig};

fn main() {
    let dir_a = std::env::temp_dir().join(format!("ghola-measure-a-{}", std::process::id()));
    let mut alice = Repo::open(&dir_a).unwrap();
    let mut files = Files::new();
    let mut parent: Vec<ObjectId> = Vec::new();
    for c in 0..40usize {
        let f = c % 12;
        files.insert(
            format!("dir{}/file{f}.txt", f % 4).into_bytes(),
            format!("rev {c}\n{}\n", "some content ".repeat(60 + (c * 17) % 200)).into_bytes(),
        );
        let tree = alice.write_tree(&files).unwrap();
        let tip = alice
            .write_commit(tree, parent.clone(), "m", &format!("c{c}"), c as u64)
            .unwrap();
        parent = vec![tip];
        alice.set_ref("main", tip).unwrap();
    }
    let stats = alice.stats();
    println!(
        "history: {} objects, {} bytes\n",
        stats.objects, stats.bytes
    );
    println!(
        "{:<28} {:>8} {:>10} {:>9} {:>9} {:>10}",
        "network", "ticks", "datagrams", "dropped", "corrupt", "vs clean"
    );
    let mut clean_ticks = 0u64;
    for (name, loss) in [
        ("clean", 0.0),
        ("10% loss (+dup/reorder/corrupt)", 0.10),
        ("20% loss", 0.20),
        ("30% loss", 0.30),
    ] {
        let dir_s = std::env::temp_dir().join(format!(
            "ghola-measure-s-{}-{}",
            std::process::id(),
            (loss * 100.0) as u32
        ));
        let mut server = Repo::open(&dir_s).unwrap();
        let cfg = if loss == 0.0 {
            NetConfig::clean(1)
        } else {
            NetConfig::hostile(1, loss)
        };
        let mut net = DistransRemote::new(&mut server, cfg);
        push(&mut alice, &mut net, "main", false).expect("push must succeed");
        let s = net.stats();
        if loss == 0.0 {
            clean_ticks = s.ticks;
        }
        println!(
            "{:<28} {:>8} {:>10} {:>9} {:>9} {:>9.1}x",
            name,
            s.ticks,
            s.client_to_server.sent + s.server_to_client.sent,
            s.client_to_server.dropped + s.server_to_client.dropped,
            s.client_to_server.corrupted + s.server_to_client.corrupted,
            s.ticks as f64 / clean_ticks as f64
        );
        drop(net);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir_s);
    }
    drop(alice);
    let _ = std::fs::remove_dir_all(&dir_a);
}
