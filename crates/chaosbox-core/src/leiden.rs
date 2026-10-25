//! Deterministic Leiden community detection (Traag, Waltman & van Eck 2019)
//! without randomness: fixed visiting order, lowest-index tie breaks and the
//! θ → 0 (greedy) refinement. The same weighted graph always yields the same
//! partition, so publication analytics stay byte-reproducible.

use std::collections::{BTreeMap, VecDeque};

struct Level {
    /// Neighbors other than the node itself, ascending, parallel edges merged.
    adj: Vec<Vec<(usize, f64)>>,
    /// Weight internal to an aggregated node (each internal edge once).
    inner: Vec<f64>,
    /// Node strength: incident weight, internal edges counted twice.
    strength: Vec<f64>,
}

impl Level {
    fn new(n: usize, edges: &[(u32, u32, f64)]) -> Self {
        let mut adj: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); n];
        for &(a, b, w) in edges {
            let (a, b) = (a as usize, b as usize);
            if a != b && a < n && b < n && w > 0.0 {
                *adj[a].entry(b).or_default() += w;
                *adj[b].entry(a).or_default() += w;
            }
        }
        Self::from_maps(adj, vec![0.0; n])
    }

    fn from_maps(adj: Vec<BTreeMap<usize, f64>>, inner: Vec<f64>) -> Self {
        let adj: Vec<Vec<(usize, f64)>> =
            adj.into_iter().map(|m| m.into_iter().collect()).collect();
        let strength = adj
            .iter()
            .zip(&inner)
            .map(|(row, inner)| row.iter().map(|(_, w)| w).sum::<f64>() + 2.0 * inner)
            .collect();
        Self {
            adj,
            inner,
            strength,
        }
    }

    fn len(&self) -> usize {
        self.adj.len()
    }

    /// Weight from `v` to each community, ascending by community index.
    fn weights_to(
        &self,
        v: usize,
        label: &[usize],
        keep: impl Fn(usize) -> bool,
    ) -> BTreeMap<usize, f64> {
        let mut out = BTreeMap::new();
        for &(u, w) in &self.adj[v] {
            if keep(u) {
                *out.entry(label[u]).or_default() += w;
            }
        }
        out
    }
}

/// Partition `n` nodes into communities maximizing modularity with
/// resolution `gamma` (1.0 = classic modularity; larger = smaller groups).
/// Returns per-node community indices, numbered by size (largest first,
/// ties by smallest member), and the partition's modularity.
#[must_use]
pub fn partition(n: usize, edges: &[(u32, u32, f64)], gamma: f64) -> (Vec<u32>, f64) {
    let base = Level::new(n, edges);
    let total: f64 = base.strength.iter().sum();
    if total == 0.0 {
        return (renumber(&(0..n).collect::<Vec<_>>()), 0.0);
    }
    let mut graph = Level::new(n, edges);
    let mut node_of: Vec<usize> = (0..n).collect();
    let mut part: Vec<usize> = (0..n).collect();
    // ponytail: 32 aggregation levels; real graphs converge in a handful.
    for _ in 0..32 {
        let moved = move_nodes(&graph, &mut part, gamma, total);
        let communities = dense(&mut part);
        if communities == graph.len() {
            break;
        }
        let mut refined = refine(&graph, &part, gamma, total);
        let clusters = dense(&mut refined);
        if !moved && clusters == graph.len() {
            break;
        }
        let mut next_part = vec![0; clusters];
        for v in 0..graph.len() {
            next_part[refined[v]] = part[v];
        }
        graph = aggregate(&graph, &refined, clusters);
        for node in &mut node_of {
            *node = refined[*node];
        }
        part = next_part;
    }
    let membership: Vec<usize> = node_of.iter().map(|&v| part[v]).collect();
    let quality = modularity(&base, &membership, gamma, total);
    (renumber(&membership), quality)
}

/// Fast local moving: visit queued nodes in FIFO order, move each to the
/// neighboring community with the strictly best gain (current community
/// first, then ascending indices), and requeue neighbors that may now move.
fn move_nodes(graph: &Level, part: &mut [usize], gamma: f64, total: f64) -> bool {
    let n = graph.len();
    let mut size = vec![0usize; n];
    let mut weight = vec![0.0; n];
    for v in 0..n {
        size[part[v]] += 1;
        weight[part[v]] += graph.strength[v];
    }
    let mut empty: Vec<usize> = (0..n).filter(|&c| size[c] == 0).rev().collect();
    let mut queue: VecDeque<usize> = (0..n).collect();
    let mut queued = vec![true; n];
    let mut moved = false;
    while let Some(v) = queue.pop_front() {
        queued[v] = false;
        let current = part[v];
        let k = graph.strength[v];
        let to = graph.weights_to(v, part, |_| true);
        size[current] -= 1;
        weight[current] -= k;
        let gain = |c: usize, w: f64| w - gamma * k * weight[c] / total;
        let mut best = current;
        let mut best_gain = gain(current, to.get(&current).copied().unwrap_or(0.0));
        for (&c, &w) in &to {
            let g = gain(c, w);
            if g > best_gain {
                best = c;
                best_gain = g;
            }
        }
        if best_gain < 0.0 && size[current] > 0 {
            // Alone beats every neighbor: open an empty community.
            if let Some(fresh) = empty.pop() {
                best = fresh;
            }
        }
        size[best] += 1;
        weight[best] += k;
        if size[current] == 0 && best != current {
            empty.push(current);
        }
        if best != current {
            part[v] = best;
            moved = true;
            for &(u, _) in &graph.adj[v] {
                if part[u] != best && !queued[u] {
                    queued[u] = true;
                    queue.push_back(u);
                }
            }
        }
    }
    moved
}

/// Leiden refinement: inside each community, well-connected singletons
/// greedily join well-connected sub-clusters with non-negative gain, so
/// every final community is internally connected.
fn refine(graph: &Level, part: &[usize], gamma: f64, total: f64) -> Vec<usize> {
    let n = graph.len();
    let mut community_weight = vec![0.0; n];
    for v in 0..n {
        community_weight[part[v]] += graph.strength[v];
    }
    let mut refined: Vec<usize> = (0..n).collect();
    let mut singleton = vec![true; n];
    let mut cluster_weight = graph.strength.clone();
    // Weight from each sub-cluster to the rest of its community.
    let mut external: Vec<f64> = (0..n)
        .map(|v| {
            graph.adj[v]
                .iter()
                .filter(|(u, _)| part[*u] == part[v])
                .map(|(_, w)| w)
                .sum()
        })
        .collect();
    let connected =
        |inside: f64, k: f64, community: f64| inside >= gamma * k * (community - k) / total;
    for v in 0..n {
        let community = community_weight[part[v]];
        let k = graph.strength[v];
        if !singleton[v] || !connected(external[v], k, community) {
            continue;
        }
        let to = graph.weights_to(v, &refined, |u| part[u] == part[v]);
        let mut best: Option<(usize, f64)> = None;
        for (&c, &w) in &to {
            if c == refined[v] || !connected(external[c], cluster_weight[c], community) {
                continue;
            }
            let gain = w - gamma * k * cluster_weight[c] / total;
            if gain >= 0.0 && best.is_none_or(|(_, g)| gain > g) {
                best = Some((c, gain));
            }
        }
        if let Some((c, _)) = best {
            external[c] += external[v] - 2.0 * to[&c];
            cluster_weight[c] += k;
            refined[v] = c;
            singleton[v] = false;
            singleton[c] = false;
        }
    }
    refined
}

fn aggregate(graph: &Level, refined: &[usize], clusters: usize) -> Level {
    let mut adj: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); clusters];
    let mut inner = vec![0.0; clusters];
    for v in 0..graph.len() {
        let a = refined[v];
        inner[a] += graph.inner[v];
        for &(u, w) in &graph.adj[v] {
            let b = refined[u];
            if a != b {
                *adj[a].entry(b).or_default() += w;
            } else if v < u {
                inner[a] += w;
            }
        }
    }
    Level::from_maps(adj, inner)
}

/// Renumber labels densely in first-appearance order; returns the count.
fn dense(labels: &mut [usize]) -> usize {
    let mut map = BTreeMap::new();
    for label in labels.iter_mut() {
        let next = map.len();
        *label = *map.entry(*label).or_insert(next);
    }
    map.len()
}

/// Number communities by size descending, ties by smallest member.
fn renumber(membership: &[usize]) -> Vec<u32> {
    let mut groups: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (v, &c) in membership.iter().enumerate() {
        let entry = groups.entry(c).or_insert((0, v));
        entry.0 += 1;
    }
    let mut ranked: Vec<_> = groups.into_iter().collect();
    ranked.sort_by(|(_, (sa, ma)), (_, (sb, mb))| sb.cmp(sa).then(ma.cmp(mb)));
    let rank: BTreeMap<usize, u32> = ranked
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (*c, u32::try_from(i).unwrap_or(u32::MAX)))
        .collect();
    membership.iter().map(|c| rank[c]).collect()
}

fn modularity(graph: &Level, membership: &[usize], gamma: f64, total: f64) -> f64 {
    let groups = membership.iter().copied().max().map_or(0, |m| m + 1);
    let mut inside = vec![0.0; groups];
    let mut strength = vec![0.0; groups];
    for v in 0..graph.len() {
        let c = membership[v];
        strength[c] += graph.strength[v];
        for &(u, w) in &graph.adj[v] {
            if membership[u] == c {
                inside[c] += w;
            }
        }
    }
    inside
        .iter()
        .zip(&strength)
        .map(|(i, s)| i / total - gamma * (s / total).powi(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::partition;

    fn cliques() -> Vec<(u32, u32, f64)> {
        let mut edges = Vec::new();
        for base in [0u32, 5] {
            for a in 0..5 {
                for b in a + 1..5 {
                    edges.push((base + a, base + b, 1.0));
                }
            }
        }
        edges.push((4, 5, 1.0));
        edges
    }

    #[test]
    fn bridged_cliques_split_deterministically_and_stay_connected() {
        let (labels, quality) = partition(11, &cliques(), 1.0);
        assert_eq!(&labels[..10], &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1]);
        assert_eq!(labels[10], 2, "isolated node is its own community");
        assert!(quality > 0.4);
        let mut shuffled = cliques();
        shuffled.reverse();
        assert_eq!(partition(11, &shuffled, 1.0).0, labels);
        // Higher resolution never merges the cliques.
        assert_eq!(partition(11, &cliques(), 2.0).0[..10], labels[..10]);
    }

    #[test]
    fn empty_and_edgeless_graphs_are_singletons() {
        assert!(partition(0, &[], 1.0).0.is_empty());
        assert_eq!(partition(3, &[], 1.0).0, vec![0, 1, 2]);
    }
}
