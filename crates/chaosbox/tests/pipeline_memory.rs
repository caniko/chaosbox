//! Explicit, offline RSS comparison against a previously built CLI. Neither
//! process connects to `TypeDB` or performs inference. Set `CHAOSBOX_BASELINE_BIN`
//! to compare the original implementation on exactly the same corpus. Override
//! the generated fixture with `CHAOSBOX_MEMORY_CORPUS` for a read-only real tree.

use std::{
    fmt::Write as _,
    fs::File,
    path::Path,
    process::{Command, Stdio},
};

use sha2::{Digest, Sha256};

#[test]
#[ignore = "offline memory benchmark; run explicitly with --nocapture"]
fn pipeline_peak_memory_and_output_parity() {
    fn run(binary: &Path, root: &Path, corpus: &Path, label: &str) -> (String, u64) {
        let graph = root.join(format!("{label}.json"));
        let metric = root.join(format!("{label}.rss"));
        let started = std::time::Instant::now();
        let status = Command::new("time")
            .args(["-f", "%M", "-o"])
            .arg(&metric)
            .arg(binary)
            .arg("run")
            .arg(corpus)
            .args([
                "--repo",
                "memory-benchmark",
                "--max-candidates",
                "40",
                "--no-decisions",
            ])
            .env("CHAOSBOX_DB_BACKEND", "memory")
            .stdout(Stdio::from(File::create(&graph).unwrap()))
            .status()
            .unwrap();
        assert!(status.success(), "{label} pipeline failed: {status}");
        let peak_kib = std::fs::read_to_string(metric)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut hash = Sha256::new();
        std::io::copy(&mut File::open(graph).unwrap(), &mut hash).unwrap();
        let digest = format!("{:x}", hash.finalize());
        eprintln!(
            "{label}: peak={peak_kib} KiB; elapsed={:?}; graph_sha256={digest}",
            started.elapsed()
        );
        (digest, peak_kib)
    }
    let root = tempfile::tempdir().unwrap();
    let corpus = if let Some(path) = std::env::var_os("CHAOSBOX_MEMORY_CORPUS") {
        std::path::PathBuf::from(path)
    } else {
        let corpus = root.path().join("corpus");
        std::fs::create_dir(&corpus).unwrap();
        let mut source = String::new();
        for n in 0..200 {
            writeln!(source, "export function function_{n}() {{ return {n}; }}").unwrap();
        }
        for file in 0..250 {
            std::fs::write(corpus.join(format!("module_{file:04}.ts")), &source).unwrap();
        }
        corpus
    };
    let baseline = std::env::var_os("CHAOSBOX_BASELINE_BIN")
        .map(|binary| run(Path::new(&binary), root.path(), &corpus, "baseline"));
    let current = run(
        Path::new(env!("CARGO_BIN_EXE_chaosbox")),
        root.path(),
        &corpus,
        "current",
    );
    if let Some(baseline) = baseline {
        assert_eq!(baseline.0, current.0, "graph export changed");
        assert!(current.1 < baseline.1, "peak RSS did not improve");
    }
}
