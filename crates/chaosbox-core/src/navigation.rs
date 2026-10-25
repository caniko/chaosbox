//! Deterministic, build-bound graph navigation and analytics. Communities
//! come from deterministic Leiden over the published relationships: they are
//! structural groupings of this build, not inferred architecture, and every
//! derived item (god node, surprise, question) cites the ids it rests on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use serde_json::{json, Value};

type Index<'a> = BTreeMap<&'a str, &'a Value>;
type Adjacency<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

/// Publication-time analytics have a bounded projection independent of the
/// source graph's size. Full source/entity/evidence records are never cached.
pub const SUMMARY_BYTES_MAX: usize = 8 * 1024 * 1024;
/// Current analytics contract; stored packets with another value are stale.
pub const ALGORITHM: &str = "leiden-v1";
/// Classic modularity. Readers recompute from the export for other values.
pub const DEFAULT_RESOLUTION: f64 = 1.0;
const SURPRISES_MAX: usize = 50;
const QUESTIONS_PER_KIND: usize = 5;
/// Plumbing relations are expected everywhere and never surprising.
const PLUMBING: [&str; 3] = ["contains", "defines", "imports"];

/// Compute immutable analytics once while publishing a build. `class_of`
/// maps a relation id to its sealed evidence class (`extracted`, `inferred`,
/// `ambiguous`), when known. The 200-entry/member limit matches the public
/// analytics read contract.
pub fn summarize_build(
    build: &crate::GraphBuild,
    class_of: impl Fn(&str) -> Option<&'static str>,
) -> Result<Value, String> {
    let mut snapshots = build.snapshot_ids.clone();
    snapshots.sort();
    let graph = json!({
        "build_id":build.id,"generation":build.generation,"snapshots":snapshots,
        "coverage":build.coverage.as_ref().map(crate::coverage::BuildCoverage::report),
        "nodes":build.nodes.values().map(|e| json!({"id":e.id,"label":e.name,
            "kind":crate::entity_kind_name(&e.kind),"source_file":e.file})).collect::<Vec<_>>(),
        "links":build.edges.values().map(|r| json!({"id":r.id,"source":r.from,"target":r.to,
            "rel_type":crate::relation_type_name(&r.rel_type),"class":class_of(&r.id)})).collect::<Vec<_>>(),
    });
    let packet = summary(&graph, 200)?;
    if serde_json::to_vec(&packet)
        .map_err(|_| "encode analytics")?
        .len()
        > SUMMARY_BYTES_MAX
    {
        return Err("publication analytics exceed byte budget".into());
    }
    Ok(packet)
}

fn index(graph: &Value) -> Result<(Index<'_>, Adjacency<'_>), String> {
    let mut nodes = BTreeMap::new();
    for node in graph["nodes"].as_array().ok_or("missing nodes")? {
        let id = node["id"].as_str().ok_or("node lacks identity")?;
        if nodes.insert(id, node).is_some() {
            return Err("duplicate node identity".into());
        }
    }
    let mut adjacency: Adjacency<'_> = nodes.keys().map(|id| (*id, BTreeSet::new())).collect();
    for edge in graph["links"].as_array().ok_or("missing links")? {
        let from = edge["source"].as_str().ok_or("edge lacks source")?;
        let to = edge["target"].as_str().ok_or("edge lacks target")?;
        adjacency.get_mut(from).ok_or("dangling source")?.insert(to);
        adjacency.get_mut(to).ok_or("dangling target")?.insert(from);
    }
    Ok((nodes, adjacency))
}

/// [`summary_at`] with the default resolution.
pub fn summary(graph: &Value, limit: usize) -> Result<Value, String> {
    summary_at(graph, limit, DEFAULT_RESOLUTION)
}

/// Repository-relative directory "area": the first two parent segments.
fn area(path: &str) -> String {
    let mut dirs: Vec<&str> = path.split('/').collect();
    dirs.pop();
    dirs.truncate(2);
    dirs.join("/")
}

fn is_doc(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        ["md", "markdown", "mdx", "txt", "rst", "adoc"]
            .iter()
            .any(|d| d.eq_ignore_ascii_case(ext))
    })
}

fn is_symbol(node: &Value) -> bool {
    matches!(node["kind"].as_str(), Some("definition" | "symbol"))
}

/// `part / whole` for graph counts, far below f64's exact-integer range.
#[allow(clippy::cast_precision_loss)]
fn ratio(part: usize, whole: usize) -> f64 {
    part as f64 / whole.max(1) as f64
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// One follow-up question and the read that answers it.
fn ask(kind: &str, text: String, ids: Vec<&str>, tool: &str, args: Value) -> Value {
    let mut question = serde_json::Map::new();
    question.insert("kind".into(), kind.into());
    question.insert("text".into(), text.into());
    question.insert("ids".into(), ids.into());
    question.insert("tool".into(), tool.into());
    question.insert("args".into(), args);
    Value::Object(question)
}

/// One graph's analytics working set, shared by the derived sections.
struct Analysis<'a> {
    graph: &'a Value,
    nodes: Index<'a>,
    adjacency: Adjacency<'a>,
    links: &'a [Value],
    /// Node id -> community rank (0 = largest).
    community: BTreeMap<&'a str, usize>,
    members: Vec<Vec<&'a str>>,
    community_ids: Vec<String>,
    labels: Vec<String>,
    cohesion: Vec<f64>,
    /// Modularity of the partition at the requested resolution.
    quality: f64,
}

impl<'a> Analysis<'a> {
    fn new(graph: &'a Value, resolution: f64) -> Result<Self, String> {
        let (nodes, adjacency) = index(graph)?;
        let links = graph["links"].as_array().ok_or("missing links")?.as_slice();
        let ids: Vec<&str> = nodes.keys().copied().collect();
        let position: BTreeMap<&str, u32> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, u32::try_from(i).unwrap_or(u32::MAX)))
            .collect();
        let edges: Vec<(u32, u32, f64)> = links
            .iter()
            .filter_map(|l| {
                let a = position.get(l["source"].as_str()?)?;
                let b = position.get(l["target"].as_str()?)?;
                Some((*a, *b, 1.0))
            })
            .collect();
        let (ranks, quality) = crate::leiden::partition(ids.len(), &edges, resolution);
        let count = ranks.iter().map(|r| *r as usize + 1).max().unwrap_or(0);
        let mut members = vec![Vec::new(); count];
        let mut community = BTreeMap::new();
        for (id, rank) in ids.iter().zip(&ranks) {
            members[*rank as usize].push(*id);
            community.insert(*id, *rank as usize);
        }
        let build_id = graph["build_id"].as_str().ok_or("missing build")?;
        let community_ids = members
            .iter()
            .map(|m| {
                let encoded = serde_json::to_string(m).map_err(|_| "encode group")?;
                Ok(crate::deterministic_id("community", &[build_id, &encoded]))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut analysis = Self {
            graph,
            nodes,
            adjacency,
            links,
            community,
            members,
            community_ids,
            labels: Vec::new(),
            cohesion: Vec::new(),
            quality,
        };
        analysis.labels = (0..count).map(|c| analysis.label(c)).collect();
        analysis.cohesion = (0..count)
            .map(|c| analysis.edge_counts(c))
            .map(|(inside, _, n)| {
                if n < 2 {
                    1.0
                } else {
                    round(ratio(inside, n * (n - 1) / 2), 3)
                }
            })
            .collect();
        Ok(analysis)
    }

    fn degree(&self, id: &str) -> usize {
        self.adjacency.get(id).map_or(0, BTreeSet::len)
    }

    fn file(&self, id: &str) -> &str {
        self.nodes[id]["source_file"].as_str().unwrap_or("")
    }

    fn name<'s>(&'s self, id: &'s str) -> &'s str {
        self.nodes[id]["label"].as_str().unwrap_or(id)
    }

    /// Hub-first members: degree descending, symbols before files, then id.
    fn ranked(&self, c: usize) -> Vec<&'a str> {
        let mut ranked = self.members[c].clone();
        ranked.sort_by(|a, b| {
            is_symbol(self.nodes[b])
                .cmp(&is_symbol(self.nodes[a]))
                .then(self.degree(b).cmp(&self.degree(a)))
                .then(a.cmp(b))
        });
        ranked
    }

    /// Deterministic label: dominant area, then the top symbol hub.
    fn label(&self, c: usize) -> String {
        let mut areas: BTreeMap<String, usize> = BTreeMap::new();
        for id in &self.members[c] {
            *areas.entry(area(self.file(id))).or_default() += 1;
        }
        let dominant = areas
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map_or(
                "(root)",
                |(a, _)| if a.is_empty() { "(root)" } else { a.as_str() },
            );
        let hub = self.ranked(c).first().map_or("", |id| self.name(id));
        format!("{dominant} · {hub}")
    }

    /// (distinct internal pairs, distinct boundary pairs, size).
    fn edge_counts(&self, c: usize) -> (usize, usize, usize) {
        let (mut inside, mut boundary) = (0, 0);
        for id in &self.members[c] {
            for next in &self.adjacency[id] {
                if self.community[next] == c {
                    inside += usize::from(id < next);
                } else {
                    boundary += 1;
                }
            }
        }
        (inside, boundary, self.members[c].len())
    }

    /// A link's evidence class; export fallbacks lack it. Builds without any
    /// decision or resolved relation are wholly EXTRACTED by publication.
    fn class(&self, link: &'a Value) -> &'a str {
        link["class"].as_str().unwrap_or_else(|| {
            let coverage = &self.graph["coverage"];
            if coverage["decision_relations"] == 0
                && coverage["resolved_relations"].as_u64().unwrap_or(0) == 0
            {
                "extracted"
            } else {
                "unknown"
            }
        })
    }

    fn communities(&self, limit: usize) -> Vec<Value> {
        (0..self.members.len())
            .take(limit)
            .map(|c| {
                let (inside, boundary, size) = self.edge_counts(c);
                let top: Vec<_> = self
                    .ranked(c)
                    .into_iter()
                    .take(5)
                    .map(|id| json!({"id":id,"label":self.name(id),"degree":self.degree(id)}))
                    .collect();
                json!({"id":self.community_ids[c],"label":self.labels[c],"size":size,
                    "internal_edges":inside,"boundary_edges":boundary,"cohesion":self.cohesion[c],
                    "top":top,"members":self.members[c].iter().take(limit).collect::<Vec<_>>(),
                    "omitted_members":size.saturating_sub(limit)})
            })
            .collect()
    }

    /// Inferred relation ids incident to each node.
    fn inferred(&self) -> BTreeMap<&'a str, Vec<&'a str>> {
        let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for link in self.links {
            if self.class(link) == "inferred" {
                if let (Some(a), Some(b), Some(id)) = (
                    link["source"].as_str(),
                    link["target"].as_str(),
                    link["id"].as_str(),
                ) {
                    out.entry(a).or_default().push(id);
                    out.entry(b).or_default().push(id);
                }
            }
        }
        out
    }

    /// Symbols ranked by degree: Graphify's god nodes without file noise.
    fn god_nodes(&self, limit: usize) -> Vec<Value> {
        let inferred = self.inferred();
        let mut ranked: Vec<&str> = self
            .nodes
            .iter()
            .filter(|(_, n)| is_symbol(n))
            .map(|(id, _)| *id)
            .collect();
        ranked.sort_by(|a, b| self.degree(b).cmp(&self.degree(a)).then(a.cmp(b)));
        ranked
            .into_iter()
            .take(limit)
            .map(|id| {
                json!({"id":id,"label":self.name(id),"kind":self.nodes[id]["kind"],
                    "source_file":self.file(id),"degree":self.degree(id),
                    "inferred_edges":inferred.get(id).map_or(0, Vec::len),
                    "community":self.community_ids[self.community[id]]})
            })
            .collect()
    }

    /// Graphify's surprise score, every term explained. A link is listed only
    /// when something besides its evidence class makes it unexpected.
    fn surprises(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for link in self.links {
            let (Some(a), Some(b)) = (link["source"].as_str(), link["target"].as_str()) else {
                continue;
            };
            let rel = link["rel_type"].as_str().unwrap_or("");
            if a == b || PLUMBING.contains(&rel) {
                continue;
            }
            let class = self.class(link);
            let mut score = 0;
            let mut why = Vec::new();
            match class {
                "ambiguous" => {
                    score += 3;
                    why.push("ambiguous evidence".to_owned());
                }
                "inferred" => {
                    score += 2;
                    why.push("inferred, not stated verbatim in source".to_owned());
                }
                "extracted" => score += 1,
                _ => {}
            }
            let base = score;
            let (fa, fb) = (self.file(a), self.file(b));
            if is_doc(fa) != is_doc(fb) {
                score += 2;
                why.push("crosses code and documentation".to_owned());
            }
            let (aa, ab) = (area(fa), area(fb));
            if aa != ab {
                score += 2;
                why.push(format!("crosses areas `{aa}` → `{ab}`"));
            }
            if self.community[a] != self.community[b] {
                score += 1;
                why.push("bridges separate communities".to_owned());
            }
            let (da, db) = (self.degree(a), self.degree(b));
            if da.min(db) <= 2 && da.max(db) >= 5 {
                let (low, high) = if da <= db { (a, b) } else { (b, a) };
                score += 1;
                why.push(format!(
                    "peripheral `{}` reaches hub `{}`",
                    self.name(low),
                    self.name(high)
                ));
            }
            if score == base {
                continue;
            }
            out.push(
                json!({"rel_id":link["id"],"rel_type":rel,"class":class,"source":a,"target":b,
                "source_label":self.name(a),"target_label":self.name(b),"score":score,"why":why}),
            );
        }
        out.sort_by(|x, y| {
            y["score"]
                .as_u64()
                .cmp(&x["score"].as_u64())
                .then_with(|| x["rel_id"].as_str().cmp(&y["rel_id"].as_str()))
        });
        out.truncate(SURPRISES_MAX);
        out
    }

    /// Follow-up questions, each carrying the exact read tool and arguments
    /// that answer it (the reader adds `repo`).
    fn questions(&self, gods: &[Value], surprises: &[Value]) -> Vec<Value> {
        let mut out = self.ambiguous_questions();
        out.extend(self.bridge_questions());
        out.extend(self.verify_questions(gods));
        out.extend(self.unused_questions());
        out.extend(self.boundary_questions());
        out.extend(surprises.first().map(|top| self.surprise_question(top)));
        out
    }

    fn ambiguous_questions(&self) -> Vec<Value> {
        self.links
            .iter()
            .filter(|l| self.class(l) == "ambiguous")
            .take(QUESTIONS_PER_KIND)
            .map(|link| {
                let (a, b) = (
                    link["source"].as_str().unwrap_or(""),
                    link["target"].as_str().unwrap_or(""),
                );
                let rel = link["id"].as_str().unwrap_or("");
                let text = format!(
                    "What is the exact relationship between `{}` and `{}`?",
                    self.name(a),
                    self.name(b)
                );
                ask(
                    "ambiguous_edge",
                    text,
                    vec![rel],
                    "evidence",
                    json!({"rel":rel}),
                )
            })
            .collect()
    }

    /// Cheap stand-in for betweenness: symbols whose neighbors span ≥ 3
    /// communities.
    fn bridge_questions(&self) -> Vec<Value> {
        let mut bridges: Vec<(usize, &str)> = self
            .nodes
            .iter()
            .filter(|(_, n)| is_symbol(n))
            .map(|(id, _)| {
                let spans: BTreeSet<_> = self.adjacency[id]
                    .iter()
                    .map(|n| self.community[n])
                    .collect();
                (spans.len(), *id)
            })
            .filter(|(spans, _)| *spans >= 3)
            .collect();
        bridges.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(self.degree(b.1).cmp(&self.degree(a.1)))
                .then(a.1.cmp(b.1))
        });
        bridges
            .into_iter()
            .take(QUESTIONS_PER_KIND)
            .map(|(spans, id)| {
                let text = format!("Why does `{}` connect {spans} communities?", self.name(id));
                ask("bridge_node", text, vec![id], "neighbors", json!({"id":id}))
            })
            .collect()
    }

    fn verify_questions(&self, gods: &[Value]) -> Vec<Value> {
        let inferred = self.inferred();
        gods.iter()
            .filter_map(|god| {
                let id = god["id"].as_str()?;
                let rels = inferred.get(id).filter(|rels| rels.len() >= 2)?;
                let text = format!(
                    "Are the {} inferred relationships of `{}` correct?",
                    rels.len(),
                    self.name(id)
                );
                let ids = rels.iter().take(10).copied().collect();
                Some(ask(
                    "verify_inferred",
                    text,
                    ids,
                    "evidence",
                    json!({"rel":rels[0]}),
                ))
            })
            .take(QUESTIONS_PER_KIND)
            .collect()
    }

    /// Function-like symbols with no resolved use, in files where resolution
    /// produced something. Test code is called by the harness.
    fn unused_questions(&self) -> Vec<Value> {
        let endpoints = |l: &'a Value| {
            [
                l["source"].as_str().unwrap_or(""),
                l["target"].as_str().unwrap_or(""),
            ]
        };
        let active: BTreeSet<&str> = self
            .links
            .iter()
            .filter(|l| !PLUMBING.contains(&l["rel_type"].as_str().unwrap_or("")))
            .flat_map(endpoints)
            .collect();
        let resolved_files: BTreeSet<&str> = active
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter_map(|n| n["source_file"].as_str())
            .collect();
        let in_tests: BTreeSet<&str> = self
            .links
            .iter()
            .filter(|l| l["rel_type"] == "contains")
            .filter(|l| {
                l["source"]
                    .as_str()
                    .is_some_and(|s| self.nodes.contains_key(s) && self.name(s) == "tests")
            })
            .filter_map(|l| l["target"].as_str())
            .collect();
        // ponytail: lowercase-first names approximate functions; type use is
        // not modeled, so unused-looking types would only add noise here.
        let mut unused: Vec<&str> = self
            .nodes
            .iter()
            .filter(|(id, n)| {
                let (file, name) = (self.file(id), self.name(id));
                is_symbol(n)
                    && !active.contains(**id)
                    && !in_tests.contains(**id)
                    && resolved_files.contains(file)
                    && name.starts_with(|c: char| c.is_lowercase())
                    && name != "tests"
                    && !file.contains("tests/")
                    && !file.ends_with("tests.rs")
            })
            .map(|(id, _)| *id)
            .collect();
        unused.sort_by(|a, b| {
            self.file(a)
                .cmp(self.file(b))
                .then(self.name(a).cmp(self.name(b)))
                .then(a.cmp(b))
        });
        unused
            .into_iter()
            .take(QUESTIONS_PER_KIND)
            .map(|id| {
                let text = format!(
                    "What uses `{}`? No resolved calls, references or heritage (absence is not proof).",
                    self.name(id)
                );
                ask("isolated_node", text, vec![id], "explain", json!({"id":id}))
            })
            .collect()
    }

    /// Density (`cohesion`, Graphify's metric) falls with size, so it flags
    /// every large group. A boundary crossed by most of a community's
    /// relationships is the size-independent sign of a weak module.
    fn boundary_questions(&self) -> Vec<Value> {
        let mut weak: Vec<(f64, usize, usize, usize)> = (0..self.members.len())
            .filter(|&c| self.members[c].len() >= 5)
            .filter_map(|c| {
                let (inside, boundary, _) = self.edge_counts(c);
                let share = ratio(inside, inside + boundary);
                (share < 0.5).then_some((share, c, inside, boundary))
            })
            .collect();
        weak.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        weak.into_iter()
            .take(QUESTIONS_PER_KIND)
            .map(|(_, c, inside, boundary)| {
                let id = self.community_ids[c].as_str();
                let text = format!(
                    "Is `{}` a real module? {boundary} of its {} relationships cross its boundary.",
                    self.labels[c],
                    inside + boundary
                );
                ask(
                    "weak_boundary",
                    text,
                    vec![id],
                    "community",
                    json!({"id":id}),
                )
            })
            .collect()
    }

    fn surprise_question(&self, top: &Value) -> Value {
        let (a, b) = (
            top["source"].as_str().unwrap_or(""),
            top["target"].as_str().unwrap_or(""),
        );
        let why: Vec<&str> = top["why"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let text = format!(
            "Why does `{}` reach `{}`? ({})",
            self.name(a),
            self.name(b),
            why.join("; ")
        );
        let rel = top["rel_id"].as_str().unwrap_or("");
        ask(
            "surprise_path",
            text,
            vec![rel],
            "path",
            json!({"from":a,"to":b}),
        )
    }

    fn edge_classes(&self) -> BTreeMap<&'a str, usize> {
        let mut out: BTreeMap<&str, usize> = ["extracted", "inferred", "ambiguous", "unknown"]
            .into_iter()
            .map(|k| (k, 0))
            .collect();
        for link in self.links {
            *out.entry(self.class(link)).or_default() += 1;
        }
        out
    }
}

/// Statistics, Leiden communities, hubs, god nodes, surprising connections
/// and follow-up questions from a complete bounded export. Group identities
/// bind to the build and member ids; callers never treat them as continuity
/// across source changes.
pub fn summary_at(graph: &Value, limit: usize, resolution: f64) -> Result<Value, String> {
    if !(1..=200).contains(&limit) {
        return Err("summary limit must be 1..200".into());
    }
    if !(0.01..=10.0).contains(&resolution) {
        return Err("resolution must be 0.01..10".into());
    }
    let analysis = Analysis::new(graph, resolution)?;
    let total_groups = analysis.members.len();
    let mut hubs: Vec<_> = analysis.nodes.iter().map(|(id,node)| json!({"id":id,"label":node["label"],"degree":analysis.degree(id),"source_file":node["source_file"]})).collect();
    hubs.sort_by(|a, b| {
        b["degree"]
            .as_u64()
            .cmp(&a["degree"].as_u64())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    hubs.truncate(limit);
    let gods = analysis.god_nodes(limit);
    let surprises = analysis.surprises();
    let questions = analysis.questions(&gods, &surprises);
    Ok(
        json!({"build_id":graph["build_id"],"generation":graph["generation"],"snapshots":graph["snapshots"],
        "nodes":analysis.nodes.len(),"edges":analysis.links.len(),
        "algorithm":ALGORITHM,"resolution":resolution,"quality":round(analysis.quality, 4),
        "community_count":total_groups,"singletons":analysis.members.iter().filter(|m| m.len() == 1).count(),
        "communities":analysis.communities(limit),
        "omitted_communities":total_groups.saturating_sub(limit),"hubs":hubs,
        "omitted_hubs":analysis.nodes.len().saturating_sub(limit),"god_nodes":gods,
        "surprises":surprises,"questions":questions,"edge_classes":analysis.edge_classes(),
        "coverage":graph["coverage"],"exhaustive":false}),
    )
}

/// One community with complete membership, for exports that must place every
/// node. Ids and labels match [`summary_at`] for the same graph.
#[derive(Clone, Debug)]
pub struct Community {
    /// Build-bound community id.
    pub id: String,
    /// Deterministic `area · top hub` label.
    pub label: String,
    /// Member entity ids, ascending.
    pub members: Vec<String>,
}

/// Complete Leiden partition of a node-link graph, largest community first.
pub fn communities(graph: &Value, resolution: f64) -> Result<Vec<Community>, String> {
    if !(0.01..=10.0).contains(&resolution) {
        return Err("resolution must be 0.01..10".into());
    }
    let analysis = Analysis::new(graph, resolution)?;
    Ok((0..analysis.members.len())
        .map(|c| Community {
            id: analysis.community_ids[c].clone(),
            label: analysis.labels[c].clone(),
            members: analysis.members[c]
                .iter()
                .map(|m| (*m).to_owned())
                .collect(),
        })
        .collect())
}

/// Inspect a build-bound community from the bounded summary.
pub fn community(graph: &Value, id: &str, limit: usize) -> Result<Value, String> {
    if !(1..=200).contains(&limit) {
        return Err("community limit must be 1..200".into());
    }
    let stats = summary(graph, 200)?;
    let group = stats["communities"]
        .as_array()
        .ok_or("missing groups")?
        .iter()
        .find(|group| group["id"] == id)
        .ok_or("community is absent or outside the bounded summary")?;
    let members: BTreeSet<_> = group["members"]
        .as_array()
        .ok_or("missing members")?
        .iter()
        .take(limit)
        .filter_map(Value::as_str)
        .collect();
    let (index, _) = index(graph)?;
    let nodes: Vec<_> = members.iter().map(|id| index[id]).collect();
    Ok(
        json!({"build_id":graph["build_id"],"id":id,"algorithm":stats["algorithm"],"label":group["label"],
        "cohesion":group["cohesion"],"size":group["size"],
        "omitted_members":group["size"].as_u64().unwrap_or(0).saturating_sub(nodes.len() as u64),"nodes":nodes,"exhaustive":false}),
    )
}

/// Lowercased query terms that can discriminate: terms under three
/// characters and English function words are dropped (Graphify's rule),
/// unless nothing would remain, so `io` or `db` still search.
#[must_use]
pub fn context_terms(query: &str) -> BTreeSet<String> {
    const STOP: [&str; 34] = [
        "the", "and", "for", "with", "that", "this", "what", "which", "who", "whom", "how", "does",
        "did", "from", "into", "are", "was", "were", "where", "when", "why", "can", "its", "there",
        "their", "then", "than", "but", "not", "all", "any", "has", "have", "been",
    ];
    let query = query.to_lowercase();
    let all: BTreeSet<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let useful: BTreeSet<String> = all
        .iter()
        .filter(|t| t.chars().count() >= 3 && !STOP.contains(&t.as_str()))
        .cloned()
        .collect();
    if useful.is_empty() {
        all
    } else {
        useful
    }
}

/// Bounded lexical seeds followed by deterministic undirected BFS. All
/// returned edges keep their original direction, type and evidence identity.
pub fn context(
    graph: &Value,
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<Value, String> {
    validate_context(query, depth, max_nodes, max_chars)?;
    render_context(graph, query, depth, max_nodes, max_chars)
}

/// Validate the shared CLI/MCP context budget before performing backend reads.
pub fn validate_context(
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<(), String> {
    if query.trim().is_empty()
        || query.len() > 2048
        || !(1..=6).contains(&depth)
        || !(1..=200).contains(&max_nodes)
        || !(256..=32_000).contains(&max_chars)
    {
        return Err(
            "context requires query <=2048 bytes, depth 1..6, nodes 1..200, chars 256..32000"
                .into(),
        );
    }
    Ok(())
}

fn render_context(
    graph: &Value,
    query: &str,
    depth: usize,
    max_nodes: usize,
    max_chars: usize,
) -> Result<Value, String> {
    let (nodes, adjacency) = index(graph)?;
    let terms = context_terms(query);
    let mut seeds: Vec<_> = nodes
        .iter()
        .filter_map(|(id, node)| {
            let name = format!(
                "{} {}",
                node["label"].as_str().unwrap_or(""),
                node["qualified_name"].as_str().unwrap_or("")
            )
            .to_lowercase();
            let score = terms
                .iter()
                .filter(|term| name.contains(term.as_str()))
                .count();
            (score > 0).then_some((score, *id))
        })
        .collect();
    seeds.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut selected = BTreeSet::new();
    let mut priority = Vec::new();
    let mut queue = VecDeque::new();
    let mut truncated = seeds.len() > max_nodes;
    for (_, id) in seeds.into_iter().take(max_nodes) {
        selected.insert(id);
        priority.push(id);
        queue.push_back((id, 0));
    }
    while let Some((id, level)) = queue.pop_front() {
        if level >= depth {
            continue;
        }
        for next in &adjacency[id] {
            if selected.contains(next) {
                continue;
            }
            if selected.len() == max_nodes {
                truncated = true;
                continue;
            }
            selected.insert(*next);
            priority.push(*next);
            queue.push_back((*next, level + 1));
        }
    }
    loop {
        let mut links: Vec<_> = graph["links"]
            .as_array()
            .ok_or("missing links")?
            .iter()
            .filter(|edge| {
                edge["source"]
                    .as_str()
                    .is_some_and(|id| selected.contains(id))
                    && edge["target"]
                        .as_str()
                        .is_some_and(|id| selected.contains(id))
            })
            .collect();
        links.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let packet = json!({"build_id":graph["build_id"],"generation":graph["generation"],"snapshots":graph["snapshots"],
            "algorithm":"lexical-bfs-v2","direction":"both","nodes":selected.iter().map(|id| nodes[id]).collect::<Vec<_>>(),
            "links":links,"truncated":truncated,"exhaustive":false});
        if serde_json::to_string(&packet)
            .map_err(|_| "encode context")?
            .chars()
            .count()
            <= max_chars
        {
            return Ok(packet);
        }
        let Some(last) = priority.pop() else {
            return Err("context metadata exceeds output budget".into());
        };
        selected.remove(last);
        truncated = true;
    }
}
